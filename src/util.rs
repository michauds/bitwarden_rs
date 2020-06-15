//
// Web Headers and caching
//
use rocket::fairing::{Fairing, Info, Kind};
use rocket::http::{ContentType, Header, HeaderMap, Method, Status};
use rocket::response::{self, Responder};
use rocket::{Data, Request, Response, Manifest};
use std::io::Cursor;

use crate::CONFIG;

pub struct AppHeaders();

#[rocket::async_trait]
impl Fairing for AppHeaders {
    fn info(&self) -> Info {
        Info {
            name: "Application Headers",
            kind: Kind::Response,
        }
    }

    async fn on_response<'a>(&'a self, _req: &'a Request<'_>, res: &'a mut Response<'_>) {
        res.set_raw_header("Feature-Policy", "accelerometer 'none'; ambient-light-sensor 'none'; autoplay 'none'; camera 'none'; encrypted-media 'none'; fullscreen 'none'; geolocation 'none'; gyroscope 'none'; magnetometer 'none'; microphone 'none'; midi 'none'; payment 'none'; picture-in-picture 'none'; sync-xhr 'self' https://haveibeenpwned.com https://twofactorauth.org; usb 'none'; vr 'none'");
        res.set_raw_header("Referrer-Policy", "same-origin");
        res.set_raw_header("X-Frame-Options", "SAMEORIGIN");
        res.set_raw_header("X-Content-Type-Options", "nosniff");
        res.set_raw_header("X-XSS-Protection", "1; mode=block");
        let csp = format!("frame-ancestors 'self' chrome-extension://nngceckbapebfimnlniiiahkandclblb moz-extension://* {};", CONFIG.allowed_iframe_ancestors());
        res.set_raw_header("Content-Security-Policy", csp);

        // Disable cache unless otherwise specified
        if !res.headers().contains("cache-control") {
            res.set_raw_header("Cache-Control", "no-cache, no-store, max-age=0");
        }
    }
}

pub struct CORS();

impl CORS {
    fn get_header(headers: &HeaderMap, name: &str) -> String {
        match headers.get_one(name) {
            Some(h) => h.to_string(),
            _ => "".to_string(),
        }
    }

    fn valid_url(url: String) -> String {
        match url.as_ref() {
            "file://" => "*".to_string(),
            _ => url,
        }
    }
}

#[rocket::async_trait]
impl Fairing for CORS {
    fn info(&self) -> Info {
        Info {
            name: "CORS",
            kind: Kind::Response,
        }
    }

    async fn on_response<'a>(&'a self, request: &'a Request<'_>, response: &'a mut Response<'_>) {
        let req_headers = request.headers();

        // We need to explicitly get the Origin header for Access-Control-Allow-Origin
        let req_allow_origin = CORS::valid_url(CORS::get_header(req_headers, "Origin"));

        response.set_header(Header::new("Access-Control-Allow-Origin", req_allow_origin));

        if request.method() == Method::Options {
            let req_allow_headers = CORS::get_header(&req_headers, "Access-Control-Request-Headers");
            let req_allow_method = CORS::get_header(&req_headers, "Access-Control-Request-Method");

            response.set_header(Header::new("Access-Control-Allow-Methods", req_allow_method));
            response.set_header(Header::new("Access-Control-Allow-Headers", req_allow_headers));
            response.set_header(Header::new("Access-Control-Allow-Credentials", "true"));
            response.set_status(Status::Ok);
            response.set_header(ContentType::Plain);
            response.set_sized_body(Cursor::new("")).await;
        }
    }
}

pub struct Cached<R>(R, &'static str);

impl<R> Cached<R> {
    pub const fn long(r: R) -> Cached<R> {
        // 7 days
        Self(r, "public, max-age=604800")
    }

    pub const fn short(r: R) -> Cached<R> {
        // 10 minutes
        Self(r, "public, max-age=600")
    }
}

#[rocket::async_trait]
impl<'r, R: 'r + Responder<'r> + Send> Responder<'r> for Cached<R> {
    async fn respond_to(self, request: &'r Request<'_>) -> response::Result<'r> {
        let mut res = self.0.respond_to(request).await?;
        res.set_raw_header("Cache-Control", self.1);
        Ok(res)
    }
}

// Log all the routes from the main paths list, and the attachments endpoint
// Effectively ignores, any static file route, and the alive endpoint
const LOGGED_ROUTES: [&str; 6] = [
    "/api",
    "/admin",
    "/identity",
    "/icons",
    "/notifications/hub/negotiate",
    "/attachments",
];

// Boolean is extra debug, when true, we ignore the whitelist above and also print the mounts
pub struct BetterLogging(pub bool);
#[rocket::async_trait]
impl Fairing for BetterLogging {
    fn info(&self) -> Info {
        Info {
            name: "Better Logging",
            kind: Kind::Launch | Kind::Request | Kind::Response,
        }
    }

    fn on_launch(&self, rocket: &Manifest) {
        if self.0 {
            info!(target: "routes", "Routes loaded:");
            let mut routes: Vec<_> = rocket.routes().collect();
            routes.sort_by_key(|r| r.uri.path());
            for route in routes {
                if route.rank < 0 {
                    info!(target: "routes", "{:<6} {}", route.method, route.uri);
                } else {
                    info!(target: "routes", "{:<6} {} [{}]", route.method, route.uri, route.rank);
                }
            }
        }

        let config = rocket.config();
        let scheme = if config.tls_enabled() { "https" } else { "http" };
        let addr = format!("{}://{}:{}", &scheme, &config.address, &config.port);
        info!(target: "start", "Rocket has launched from {}", addr);
    }

    async fn on_request<'a>(&'a self, request: &'a mut Request<'_>, _data: &'a Data) {
        let method = request.method();
        if !self.0 && method == Method::Options {
            return;
        }
        let uri = request.uri();
        let uri_path = uri.path();
        // FIXME: trim_start_matches() could result in over-trimming in pathological cases;
        // strip_prefix() would be a better option once it's stable.
        let uri_subpath = uri_path.trim_start_matches(&CONFIG.domain_path());
        if self.0 || LOGGED_ROUTES.iter().any(|r| uri_subpath.starts_with(r)) {
            match uri.query() {
                Some(q) => info!(target: "request", "{} {}?{}", method, uri_path, &q[..q.len().min(30)]),
                None => info!(target: "request", "{} {}", method, uri_path),
            };
        }
    }

    async fn on_response<'a>(&'a self, request: &'a Request<'_>, response: &'a mut Response<'_>) {
        if !self.0 && request.method() == Method::Options {
            return;
        }
        // FIXME: trim_start_matches() could result in over-trimming in pathological cases;
        // strip_prefix() would be a better option once it's stable.
        let uri_subpath = request.uri().path().trim_start_matches(&CONFIG.domain_path());
        if self.0 || LOGGED_ROUTES.iter().any(|r| uri_subpath.starts_with(r)) {
            let status = response.status();
            if let Some(ref route) = request.route() {
                info!(target: "response", "{} => {} {}", route, status.code, status.reason)
            } else {
                info!(target: "response", "{} {}", status.code, status.reason)
            }
        }
    }
}

//
// File handling
//
use std::fs::{self, File};
use std::io::{Read, Result as IOResult};
use std::path::Path;

pub fn file_exists(path: &str) -> bool {
    Path::new(path).exists()
}

pub fn read_file(path: &str) -> IOResult<Vec<u8>> {
    let mut contents: Vec<u8> = Vec::new();

    let mut file = File::open(Path::new(path))?;
    file.read_to_end(&mut contents)?;

    Ok(contents)
}

pub fn write_file(path: &str, content: &[u8]) -> Result<(), crate::error::Error> {
    use std::io::Write;
    let mut f = File::create(path)?;
    f.write_all(content)?;
    f.flush()?;
    Ok(())
}

pub fn read_file_string(path: &str) -> IOResult<String> {
    let mut contents = String::new();

    let mut file = File::open(Path::new(path))?;
    file.read_to_string(&mut contents)?;

    Ok(contents)
}

pub fn delete_file(path: &str) -> IOResult<()> {
    let res = fs::remove_file(path);

    if let Some(parent) = Path::new(path).parent() {
        // If the directory isn't empty, this returns an error, which we ignore
        // We only want to delete the folder if it's empty
        fs::remove_dir(parent).ok();
    }

    res
}

const UNITS: [&str; 6] = ["bytes", "KB", "MB", "GB", "TB", "PB"];

pub fn get_display_size(size: i32) -> String {
    let mut size: f64 = size.into();
    let mut unit_counter = 0;

    loop {
        if size > 1024. {
            size /= 1024.;
            unit_counter += 1;
        } else {
            break;
        }
    }

    format!("{:.2} {}", size, UNITS[unit_counter])
}

pub fn get_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

//
// String util methods
//

use std::ops::Try;
use std::str::FromStr;

pub fn upcase_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

pub fn try_parse_string<S, T, U>(string: impl Try<Ok = S, Error = U>) -> Option<T>
where
    S: AsRef<str>,
    T: FromStr,
{
    if let Ok(Ok(value)) = string.into_result().map(|s| s.as_ref().parse::<T>()) {
        Some(value)
    } else {
        None
    }
}

//
// Env methods
//

use std::env;

pub fn get_env<V>(key: &str) -> Option<V>
where
    V: FromStr,
{
    try_parse_string(env::var(key))
}

const TRUE_VALUES: &[&str] = &["true", "t", "yes", "y", "1"];
const FALSE_VALUES: &[&str] = &["false", "f", "no", "n", "0"];

pub fn get_env_bool(key: &str) -> Option<bool> {
    match env::var(key) {
        Ok(val) if TRUE_VALUES.contains(&val.to_lowercase().as_ref()) => Some(true),
        Ok(val) if FALSE_VALUES.contains(&val.to_lowercase().as_ref()) => Some(false),
        _ => None,
    }
}

//
// Date util methods
//

use chrono::NaiveDateTime;

const DATETIME_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.6fZ";

pub fn format_date(date: &NaiveDateTime) -> String {
    date.format(DATETIME_FORMAT).to_string()
}

//
// Deserialization methods
//

use std::fmt;

use serde::de::{self, DeserializeOwned, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{self, Value};

pub type JsonMap = serde_json::Map<String, Value>;

#[derive(PartialEq, Serialize, Deserialize)]
pub struct UpCase<T: DeserializeOwned> {
    #[serde(deserialize_with = "upcase_deserialize")]
    #[serde(flatten)]
    pub data: T,
}

// https://github.com/serde-rs/serde/issues/586
pub fn upcase_deserialize<'de, T, D>(deserializer: D) -> Result<T, D::Error>
where
    T: DeserializeOwned,
    D: Deserializer<'de>,
{
    let d = deserializer.deserialize_any(UpCaseVisitor)?;
    T::deserialize(d).map_err(de::Error::custom)
}

struct UpCaseVisitor;

impl<'de> Visitor<'de> for UpCaseVisitor {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("an object or an array")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut result_map = JsonMap::new();

        while let Some((key, value)) = map.next_entry()? {
            result_map.insert(upcase_first(key), upcase_value(value));
        }

        Ok(Value::Object(result_map))
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut result_seq = Vec::<Value>::new();

        while let Some(value) = seq.next_element()? {
            result_seq.push(upcase_value(value));
        }

        Ok(Value::Array(result_seq))
    }
}

fn upcase_value(value: Value) -> Value {
    if let Value::Object(map) = value {
        let mut new_value = json!({});

        for (key, val) in map.into_iter() {
            let processed_key = _process_key(&key);
            new_value[processed_key] = upcase_value(val);
        }
        new_value
    } else if let Value::Array(array) = value {
        // Initialize array with null values
        let mut new_value = json!(vec![Value::Null; array.len()]);

        for (index, val) in array.into_iter().enumerate() {
            new_value[index] = upcase_value(val);
        }
        new_value
    } else {
        value
    }
}

fn _process_key(key: &str) -> String {
    match key.to_lowercase().as_ref() {
        "ssn" => "SSN".into(),
        _ => self::upcase_first(key),
    }
}

//
// Retry methods
//

pub fn retry<F, T, E>(func: F, max_tries: i32) -> Result<T, E>
where
    F: Fn() -> Result<T, E>,
{
    use std::{thread::sleep, time::Duration};
    let mut tries = 0;

    loop {
        match func() {
            ok @ Ok(_) => return ok,
            err @ Err(_) => {
                tries += 1;

                if tries >= max_tries {
                    return err;
                }

                sleep(Duration::from_millis(500));
            }
        }
    }
}
