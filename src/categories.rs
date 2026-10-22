//! Categorías de limpieza y validación de sus carpetas raíz.

use crate::paths::{root_is_safe, temp_name_ok};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const DAY: Duration = Duration::from_secs(86_400);

pub struct Env {
    pub temp: Option<PathBuf>,
    pub system_root: Option<PathBuf>,
    pub local_app_data: Option<PathBuf>,
    pub program_data: Option<PathBuf>,
    pub protected: Vec<String>,
    pub forbidden_trees: Vec<String>,
    pub is_admin: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CategoryId {
    UserTemp,
    WindowsTemp,
    RecycleBin,
    Thumbnails,
    Chrome,
    Edge,
    Firefox,
    DumpsLogs,
    WindowsUpdate,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Filter {
    OlderThan(Duration),
    Thumbcache,
    All,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Ready,
    NeedsAdmin,
    NotFound,
    Unrecognized,
}

#[derive(Clone, Debug)]
pub struct Category {
    pub id: CategoryId,
    pub name: &'static str,
    pub filter: Filter,
    pub roots: Vec<PathBuf>,
    pub status: Status,
}

/// Enlace simbólico o, en Windows, cualquier punto de reanálisis (junction...).
pub fn is_link(md: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if md.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    md.file_type().is_symlink()
}

fn is_real_dir(p: &Path) -> bool {
    std::fs::symlink_metadata(p).map(|md| md.is_dir() && !is_link(&md)).unwrap_or(false)
}

/// Subcarpetas directas de `dir` (vacío si no se puede leer).
fn subdirs(dir: &Path) -> Vec<PathBuf> {
    if !is_real_dir(dir) {
        return Vec::new();
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    rd.flatten().map(|e| e.path()).filter(|p| is_real_dir(p)).collect()
}

/// Perfiles `Default` y `Profile N` de un navegador Chromium, con sus carpetas de caché.
fn chromium_roots(user_data: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    for p in subdirs(user_data) {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name == "Default" || name.starts_with("Profile ") {
            v.extend(["Cache", "Code Cache", "GPUCache"].map(|c| p.join(c)));
        }
    }
    v
}

fn candidates(id: CategoryId, env: &Env) -> Vec<PathBuf> {
    let local = |rel: &[&str]| env.local_app_data.as_ref().map(|b| rel.iter().fold(b.clone(), |p, s| p.join(s)));
    let sys = |rel: &[&str]| env.system_root.as_ref().map(|b| rel.iter().fold(b.clone(), |p, s| p.join(s)));
    let data = |rel: &[&str]| env.program_data.as_ref().map(|b| rel.iter().fold(b.clone(), |p, s| p.join(s)));
    let one = |p: Option<PathBuf>| p.into_iter().collect::<Vec<_>>();
    match id {
        CategoryId::UserTemp => one(env.temp.clone()),
        CategoryId::WindowsTemp => one(sys(&["Temp"])),
        CategoryId::RecycleBin => Vec::new(),
        CategoryId::Thumbnails => one(local(&["Microsoft", "Windows", "Explorer"])),
        CategoryId::Chrome => local(&["Google", "Chrome", "User Data"]).map(|p| chromium_roots(&p)).unwrap_or_default(),
        CategoryId::Edge => local(&["Microsoft", "Edge", "User Data"]).map(|p| chromium_roots(&p)).unwrap_or_default(),
        CategoryId::Firefox => local(&["Mozilla", "Firefox", "Profiles"])
            .map(|p| subdirs(&p).into_iter().map(|d| d.join("cache2")).collect())
            .unwrap_or_default(),
        CategoryId::DumpsLogs => {
            let mut v = vec![
                local(&["CrashDumps"]),
                local(&["Microsoft", "Windows", "WER", "ReportArchive"]),
                local(&["Microsoft", "Windows", "WER", "ReportQueue"]),
            ];
            if env.is_admin {
                v.extend([
                    data(&["Microsoft", "Windows", "WER", "ReportArchive"]),
                    data(&["Microsoft", "Windows", "WER", "ReportQueue"]),
                    sys(&["Minidump"]),
                    sys(&["Logs", "CBS"]),
                ]);
            }
            v.into_iter().flatten().collect()
        }
        CategoryId::WindowsUpdate => one(sys(&["SoftwareDistribution", "Download"])),
    }
}

pub fn build_categories(env: &Env) -> Vec<Category> {
    use CategoryId::*;
    let defs = [
        (UserTemp, "Temporales del usuario", Filter::OlderThan(DAY)),
        (WindowsTemp, "Temporales de Windows", Filter::OlderThan(DAY)),
        (RecycleBin, "Papelera de reciclaje", Filter::All),
        (Thumbnails, "Caché de miniaturas", Filter::Thumbcache),
        (Chrome, "Caché de Chrome", Filter::All),
        (Edge, "Caché de Edge", Filter::All),
        (Firefox, "Caché de Firefox", Filter::All),
        (DumpsLogs, "Volcados de errores y logs antiguos", Filter::OlderThan(30 * DAY)),
        (WindowsUpdate, "Caché de Windows Update", Filter::All),
    ];
    defs.into_iter()
        .map(|(id, name, filter)| {
            let (roots, status) = if id == RecycleBin {
                (Vec::new(), Status::Ready)
            } else if matches!(id, WindowsTemp | WindowsUpdate) && !env.is_admin {
                (Vec::new(), Status::NeedsAdmin)
            } else {
                let existing: Vec<PathBuf> = candidates(id, env).into_iter().filter(|p| is_real_dir(p)).collect();
                let any = !existing.is_empty();
                let safe: Vec<PathBuf> = existing
                    .into_iter()
                    .filter(|p| {
                        p.to_str().is_some_and(|s| {
                            root_is_safe(s, &env.protected, &env.forbidden_trees) && (id != UserTemp || temp_name_ok(s))
                        })
                    })
                    .collect();
                let status = match (safe.is_empty(), any) {
                    (false, _) => Status::Ready,
                    (true, true) => Status::Unrecognized,
                    (true, false) => Status::NotFound,
                };
                (safe, status)
            };
            Category { id, name, filter, roots, status }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;
    use std::fs::create_dir_all;

    /// Perfil falso: devuelve (U, Env) con U/Documents y U/AppData/Local creadas.
    fn fake(tag: &str) -> (PathBuf, Env) {
        let u = temp_dir(tag).join("U");
        create_dir_all(u.join("Documents")).unwrap();
        create_dir_all(u.join("AppData/Local")).unwrap();
        let s = |p: PathBuf| p.to_str().unwrap().to_string();
        let env = Env {
            temp: None,
            system_root: None,
            local_app_data: Some(u.join("AppData/Local")),
            program_data: None,
            protected: vec![s(u.clone())],
            forbidden_trees: vec![s(u.join("Documents"))],
            is_admin: false,
        };
        (u, env)
    }

    fn get(cats: &[Category], id: CategoryId) -> &Category {
        cats.iter().find(|c| c.id == id).unwrap()
    }

    #[test]
    fn builds_nine_categories_in_order() {
        use CategoryId::*;
        let (_, env) = fake("cat-nine");
        let cats = build_categories(&env);
        let ids: Vec<_> = cats.iter().map(|c| c.id).collect();
        assert_eq!(
            ids,
            [UserTemp, WindowsTemp, RecycleBin, Thumbnails, Chrome, Edge, Firefox, DumpsLogs, WindowsUpdate]
        );
        assert_eq!(cats[0].name, "Temporales del usuario");
    }

    #[test]
    fn user_temp_ready_when_safe() {
        let (u, mut env) = fake("cat-ready");
        let t = u.join("AppData/Local/Temp");
        create_dir_all(&t).unwrap();
        env.temp = Some(t.clone());
        let cats = build_categories(&env);
        let c = get(&cats, CategoryId::UserTemp);
        assert_eq!(c.status, Status::Ready);
        assert_eq!(c.roots, [t]);
    }

    #[test]
    fn temp_pointing_to_profile_is_unrecognized() {
        let (u, mut env) = fake("cat-profile");
        env.temp = Some(u);
        let cats = build_categories(&env);
        let c = get(&cats, CategoryId::UserTemp);
        assert_eq!(c.status, Status::Unrecognized);
        assert!(c.roots.is_empty());
    }

    #[test]
    fn temp_inside_documents_is_unrecognized() {
        let (u, mut env) = fake("cat-docs");
        let t = u.join("Documents/Temp");
        create_dir_all(&t).unwrap();
        env.temp = Some(t);
        let cats = build_categories(&env);
        assert_eq!(get(&cats, CategoryId::UserTemp).status, Status::Unrecognized);
    }

    #[test]
    fn missing_dirs_are_not_found() {
        let (_, env) = fake("cat-missing");
        let cats = build_categories(&env);
        assert_eq!(get(&cats, CategoryId::UserTemp).status, Status::NotFound);
        assert_eq!(get(&cats, CategoryId::Chrome).status, Status::NotFound);
    }

    #[test]
    fn admin_categories_need_admin() {
        let (u, mut env) = fake("cat-admin");
        let sr = u.parent().unwrap().join("Win");
        create_dir_all(sr.join("Temp")).unwrap();
        create_dir_all(sr.join("SoftwareDistribution/Download")).unwrap();
        env.system_root = Some(sr);
        let cats = build_categories(&env);
        assert_eq!(get(&cats, CategoryId::WindowsTemp).status, Status::NeedsAdmin);
        assert_eq!(get(&cats, CategoryId::WindowsUpdate).status, Status::NeedsAdmin);
        env.is_admin = true;
        let cats = build_categories(&env);
        assert_eq!(get(&cats, CategoryId::WindowsTemp).status, Status::Ready);
        assert_eq!(get(&cats, CategoryId::WindowsUpdate).status, Status::Ready);
    }

    #[test]
    fn chrome_profiles_only_cache_dirs() {
        let (u, env) = fake("cat-chrome");
        let ud = u.join("AppData/Local/Google/Chrome/User Data");
        for d in [
            "Default/Cache",
            "Default/Code Cache",
            "Default/Network",
            "Profile 1/GPUCache",
            "System Profile/Cache",
        ] {
            create_dir_all(ud.join(d)).unwrap();
        }
        let cats = build_categories(&env);
        let mut got = get(&cats, CategoryId::Chrome).roots.clone();
        got.sort();
        let mut want = vec![ud.join("Default/Cache"), ud.join("Default/Code Cache"), ud.join("Profile 1/GPUCache")];
        want.sort();
        assert_eq!(got, want);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_user_data_or_profile_is_ignored() {
        use std::os::unix::fs::symlink;
        let (u, env) = fake("cat-link");
        let real = u.join("real");
        create_dir_all(real.join("Default/Cache")).unwrap();
        let chrome = u.join("AppData/Local/Google/Chrome");
        create_dir_all(&chrome).unwrap();
        symlink(&real, chrome.join("User Data")).unwrap();
        let cats = build_categories(&env);
        assert_eq!(get(&cats, CategoryId::Chrome).status, Status::NotFound);

        let edge = u.join("AppData/Local/Microsoft/Edge/User Data");
        create_dir_all(&edge).unwrap();
        symlink(real.join("Default"), edge.join("Default")).unwrap();
        let cats = build_categories(&env);
        assert_eq!(get(&cats, CategoryId::Edge).status, Status::NotFound);
    }

    #[test]
    fn dumps_admin_roots_only_when_admin() {
        let (u, mut env) = fake("cat-dumps");
        let sr = u.parent().unwrap().join("Win");
        create_dir_all(sr.join("Minidump")).unwrap();
        let crash = u.join("AppData/Local/CrashDumps");
        create_dir_all(&crash).unwrap();
        env.system_root = Some(sr.clone());
        let cats = build_categories(&env);
        assert_eq!(get(&cats, CategoryId::DumpsLogs).roots, [crash.clone()]);
        env.is_admin = true;
        let cats = build_categories(&env);
        let roots = &get(&cats, CategoryId::DumpsLogs).roots;
        assert!(roots.contains(&crash) && roots.contains(&sr.join("Minidump")));
    }

    #[test]
    fn recycle_bin_always_ready() {
        let (_, env) = fake("cat-bin");
        let cats = build_categories(&env);
        assert_eq!(get(&cats, CategoryId::RecycleBin).status, Status::Ready);
    }
}
