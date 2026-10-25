//! VDF/ACF de Steam y JSON mínimo de Epic. Sin dependencias; nunca hacen `panic` con basura.

const MAX_DEPTH: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub enum Vdf {
    Str(String),
    Map(Vec<(String, Vdf)>),
}

impl Vdf {
    /// Valor de una clave (sin distinguir mayúsculas); solo en mapas.
    pub fn get(&self, key: &str) -> Option<&Vdf> {
        match self {
            Vdf::Map(v) => v.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, x)| x),
            Vdf::Str(_) => None,
        }
    }

    pub fn str(&self) -> Option<&str> {
        match self {
            Vdf::Str(s) => Some(s),
            Vdf::Map(_) => None,
        }
    }
}

enum Tok {
    S(String),
    Open,
    Close,
}

fn vdf_tokens(text: &str) -> Option<Vec<Tok>> {
    let mut out = Vec::new();
    let mut it = text.trim_start_matches('\u{feff}').chars().peekable();
    while let Some(c) = it.next() {
        match c {
            c if c.is_whitespace() => {}
            '{' => out.push(Tok::Open),
            '}' => out.push(Tok::Close),
            '/' if it.peek() == Some(&'/') => {
                for c in it.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            '"' => {
                let mut s = String::new();
                loop {
                    match it.next()? {
                        '"' => break,
                        '\\' => s.push(match it.next()? {
                            'n' => '\n',
                            't' => '\t',
                            o => o, // `\\`, `\"` y lo demás tal cual
                        }),
                        o => s.push(o),
                    }
                }
                out.push(Tok::S(s));
            }
            _ => return None,
        }
    }
    Some(out)
}

fn vdf_map(t: &[Tok], i: &mut usize, depth: usize) -> Option<Vec<(String, Vdf)>> {
    if depth > MAX_DEPTH {
        return None;
    }
    let mut v = Vec::new();
    loop {
        match t.get(*i) {
            None => return if depth == 0 { Some(v) } else { None },
            Some(Tok::Close) => {
                return if depth == 0 { None } else { *i += 1; Some(v) };
            }
            Some(Tok::Open) => return None,
            Some(Tok::S(k)) => {
                *i += 1;
                let val = match t.get(*i)? {
                    Tok::S(s) => Vdf::Str(s.clone()),
                    Tok::Open => {
                        *i += 1;
                        Vdf::Map(vdf_map(t, i, depth + 1)?)
                    }
                    Tok::Close => return None,
                };
                if matches!(val, Vdf::Str(_)) {
                    *i += 1;
                }
                v.push((k.clone(), val));
            }
        }
    }
}

/// El documento es un mapa con las claves de primer nivel; `None` si está vacío o mal formado.
pub fn vdf(text: &str) -> Option<Vdf> {
    let t = vdf_tokens(text)?;
    let mut i = 0;
    let v = vdf_map(&t, &mut i, 0)?;
    (!v.is_empty()).then_some(Vdf::Map(v))
}

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(v) => v.iter().find(|(k, _)| k == key).map(|(_, x)| x),
            _ => None,
        }
    }
}

struct P<'a> {
    b: &'a [u8],
    i: usize,
}

impl P<'_> {
    fn ws(&mut self) {
        while self.b.get(self.i).is_some_and(|c| c.is_ascii_whitespace()) {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: u8) -> Option<()> {
        self.ws();
        (self.b.get(self.i) == Some(&c)).then(|| self.i += 1)
    }

    fn lit(&mut self, s: &str, v: Json) -> Option<Json> {
        self.b.get(self.i..)?.starts_with(s.as_bytes()).then(|| {
            self.i += s.len();
            v
        })
    }

    fn hex4(&mut self) -> Option<u32> {
        let h = std::str::from_utf8(self.b.get(self.i..self.i + 4)?).ok()?;
        let n = u32::from_str_radix(h, 16).ok()?;
        self.i += 4;
        Some(n)
    }

    fn string(&mut self) -> Option<String> {
        self.eat(b'"')?;
        let mut out = Vec::new();
        loop {
            let c = *self.b.get(self.i)?;
            self.i += 1;
            match c {
                b'"' => return String::from_utf8(out).ok(),
                b'\\' => {
                    let e = *self.b.get(self.i)?;
                    self.i += 1;
                    let ch = match e {
                        b'n' => '\n',
                        b't' => '\t',
                        b'r' => '\r',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'"' | b'\\' | b'/' => e as char,
                        b'u' => {
                            let hi = self.hex4()?;
                            let cp = if (0xD800..0xDC00).contains(&hi) {
                                if self.b.get(self.i..self.i + 2)? != b"\\u" {
                                    return None;
                                }
                                self.i += 2;
                                let lo = self.hex4()?;
                                if !(0xDC00..0xE000).contains(&lo) {
                                    return None;
                                }
                                0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                            } else {
                                hi
                            };
                            char::from_u32(cp)?
                        }
                        _ => return None,
                    };
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                }
                c if c < 0x20 => return None,
                c => out.push(c),
            }
        }
    }

    fn value(&mut self, depth: usize) -> Option<Json> {
        if depth > MAX_DEPTH {
            return None;
        }
        self.ws();
        match *self.b.get(self.i)? {
            b'{' => {
                self.i += 1;
                let mut v = Vec::new();
                if self.eat(b'}').is_some() {
                    return Some(Json::Obj(v));
                }
                loop {
                    let k = self.string()?;
                    self.eat(b':')?;
                    v.push((k, self.value(depth + 1)?));
                    if self.eat(b',').is_none() {
                        self.eat(b'}')?;
                        return Some(Json::Obj(v));
                    }
                }
            }
            b'[' => {
                self.i += 1;
                let mut v = Vec::new();
                if self.eat(b']').is_some() {
                    return Some(Json::Arr(v));
                }
                loop {
                    v.push(self.value(depth + 1)?);
                    if self.eat(b',').is_none() {
                        self.eat(b']')?;
                        return Some(Json::Arr(v));
                    }
                }
            }
            b'"' => self.string().map(Json::Str),
            b't' => self.lit("true", Json::Bool(true)),
            b'f' => self.lit("false", Json::Bool(false)),
            b'n' => self.lit("null", Json::Null),
            _ => {
                let s = self.i;
                while self.b.get(self.i).is_some_and(|c| matches!(c, b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E')) {
                    self.i += 1;
                }
                let n: f64 = std::str::from_utf8(&self.b[s..self.i]).ok()?.parse().ok()?;
                Some(Json::Num(n)) // `1e400` da inf; quien lo use lo valida
            }
        }
    }
}

pub fn json(text: &str) -> Option<Json> {
    let mut p = P { b: text.trim_start_matches('\u{feff}').as_bytes(), i: 0 };
    let v = p.value(0)?;
    p.ws();
    (p.i == p.b.len()).then_some(v)
}
