#[macro_use]
extern crate serde_derive;

extern crate hyper;
extern crate hyper_tls;
extern crate serde;
extern crate serde_json;
extern crate clap;
extern crate chrono;
extern crate rpassword;
extern crate futures;
extern crate tokio_core;
extern crate cookie;
extern crate regex;

use std::io::{self, Read, Write};
use std::collections::HashMap;
use std::fs;
use std::result::Result;
use std::error::Error;
use clap::{App};
use chrono::prelude::*;
use rpassword::read_password;
use futures::{Future, Stream};
use tokio_core::reactor::Core;
use std::str;
use regex::Regex;

const CONFIG_FILE_NAME: &'static str = "meazure.config.json";
const DATE_FMT: &'static str = "%Y-%m-%d";

type SslClient = hyper::Client<hyper_tls::HttpsConnector<hyper::client::HttpConnector>>;
type IoFuture<T> = Box<Future<Item=T, Error=hyper::Error>>;

fn main() {
    let msg = format!("-f, --from=[FROM] 'From date e.g. 2016-01-01 Defaults to the begging of the month'
                       -t, --to=[TO] 'To date e.g. 2016-01-31 Defaults to the end of the month'
                       -c, --config=[CONFIG] 'Config file. Defaults to {}'", CONFIG_FILE_NAME);
    let matches = App::new("meazure")
                        .version("1.0")
                        .author("Mike Nelson")
                        .about("Summarizes meazure hours by project and adds some projections.")
                        .args_from_usage(&msg)
                        .get_matches();

    let local_now = Local::now();
    let config_file_name = matches.value_of("config").unwrap_or(CONFIG_FILE_NAME);

    let from_date = match matches.value_of("from") {
        Some(f) => f.to_string(),
        None => local_now.with_day(1).unwrap().format(DATE_FMT).to_string(),
    };
    
    let to_date;
    match matches.value_of("to") {
        Some(f) => to_date = f.to_string(),
        None => {
            let last_day = last_day_of_month(local_now.year(), local_now.month());
            to_date = local_now.with_day(last_day).unwrap().format(DATE_FMT).to_string();
        },
    };

    let config;
    match get_config(config_file_name) {
        Result::Ok(cfg) => config = cfg,
        Result::Err(_) => config = create_config(config_file_name),
    }

    println!("{}: -f {} -t {}", config.uname, from_date, to_date);
    
    let mut core = Core::new().unwrap();

    let entries;
    match query_meazure(& mut core, &config, &from_date, &to_date) {
        Result::Ok(v) => entries = v,
        Result::Err(e) => {
            println!("Error querying measure: {:?}", e);
            std::process::exit(0);
        }
    };
    if entries.len() < 1 {
        println!("No entries for that date range");
        std::process::exit(0);
    }

    let agg = aggregate_hours(&entries, &config);
    let proj;
    {
        let total = agg.get("total").unwrap(); 
        proj = make_projections(&entries, total, &from_date, &to_date);  
    } 
    
    let results = Results {
        hours: agg,
        projections: proj
    };
    println!("{}", serde_json::to_string_pretty(&results).unwrap());
}

#[derive(Serialize, Deserialize)]
struct Config {
    uname: String,
    pword: String,
    rates: HashMap<String, f32>,
}

#[derive(Serialize, Deserialize)]
struct Projections {
    week_days: i64,
    week_days_past: i64,
    percent_complete: i64, 
    avg_earnings_per_day: f32,
    avg_hours_per_day: f32,
    estimated_earnings: f32,
    estimated_hours: f32
}

#[derive(Serialize, Deserialize)]
struct Earnings {
    hours: f32,
    earnings: f32
}

#[derive(Serialize, Deserialize)]
struct Results {
    hours: HashMap<String, Earnings>,
    projections: Projections
}

#[derive(Serialize, Deserialize)]
struct MeazureEntry {
    #[serde(rename = "billToDate")]
    bill_to_date: String,
    #[serde(rename = "durationMinutes")]
    duration_minutes: i32,
    #[serde(rename = "categoryName")]
    category_name: String,
    id: String,
    #[serde(rename = "isUnlocked")]
    is_unlocked: bool,
    description: String,
    #[serde(rename = "projectId")]
    project_id: String,
    #[serde(rename = "projectName")]
    project_name: String
}

#[derive(Serialize, Deserialize)]
struct MeazureResponse {
    data: Vec<MeazureEntry>,
    #[serde(rename = "recordCount")]
    record_count: i32
}

struct Entry {
    project: String,
    hours: f64,
    date: String
}

struct RequestTokens {
    cookie: String,
    field: String
}

fn get_config(config_file_name: &str) -> io::Result<Config> {
    let mut f = fs::File::open(config_file_name)?;
    let mut data = String::new();
    f.read_to_string(&mut data)?;
    let config: Config = serde_json::from_str(&data).unwrap();
    Result::Ok(config)
}

fn create_config(config_file_name: &str) -> Config {
    let stdin = io::stdin();
    let mut uname = String::new();
    
    io::stdout().write(b"Meazure username: ").unwrap();
    io::stdout().flush().unwrap();
    stdin.read_line(&mut uname).unwrap();
    
    io::stdout().write(b"Meazure password: ").unwrap();
    io::stdout().flush().unwrap();
    let pword = read_password().unwrap();

    let config = Config { 
        uname: uname.trim().to_string(), 
        pword: pword.trim().to_string(), 
        rates: HashMap::new() 
    };

    let encoded = serde_json::to_string_pretty(&config).unwrap();
    let mut f = fs::File::create(config_file_name).unwrap();
    f.write(encoded.as_bytes()).unwrap();
    return config;
}

fn last_day_of_month(year: i32, month: u32) -> u32 {
    NaiveDate::from_ymd_opt(year, month + 1, 1).unwrap_or(NaiveDate::from_ymd(year + 1, 1, 1)).pred().day()
}

// Makes an initial request and returns the request verification tokens
fn get_request_tokens(client: &SslClient) -> IoFuture<RequestTokens> {
    let uri = "https://surge.meazure.com/Account/Login".parse().unwrap();
    let response = client.get(uri)
        .and_then(|res| {
            let cookie_string: Option<String> = res.headers().get::<hyper::header::SetCookie>()
                .and_then(|c| {c.get(0)})
                .map(|c| {
                    let parsed = cookie::Cookie::parse(c.to_string()).unwrap();
                    return parsed.value().to_string();
                });

            let tokens = res.body().concat2()
                .map(|chunk| {
                    let s = str::from_utf8(&*chunk).unwrap();
                    let re = Regex::new(r#"<input name="__RequestVerificationToken".*="(.*)".*/>"#).unwrap();
                    let captures: Vec<regex::Captures> = re.captures_iter(s).collect();
                    let field = captures
                        .get(1)
                        .and_then(|c| c.get(1))
                        .map(|m| String::from(m.as_str()));// Second one
                    RequestTokens {
                        cookie: cookie_string.unwrap(),
                        field: field.unwrap()
                    }
                });
            tokens
        });
    return Box::new(response);
}

// Logs in and returns the cookies necessary to make API calls
fn login(client: &SslClient, tokens: &RequestTokens, config: &Config) -> IoFuture<hyper::header::Cookie> {
    let uri = "https://surge.meazure.com/account/login".parse().unwrap();
    let mut req = hyper::Request::new(hyper::Method::Post, uri);

    let mut cookie_header = hyper::header::Cookie::new();
    cookie_header.set("__RequestVerificationToken", tokens.cookie.clone());

    req.headers_mut().set(hyper::header::ContentType::form_url_encoded());
    req.headers_mut().set(cookie_header);
    req.set_body(format!("email={}&password={}&__RequestVerificationToken={}", config.uname, config.pword, tokens.field));

    let response = client.request(req)
        .map(|res| {
            let cookies: Option<hyper::header::Cookie> = res.headers().get::<hyper::header::SetCookie>()
                .map(|c| {c.iter().filter(|s| {s.contains("AspNet.Cookies")}).collect()})
                .map(|c: Vec<&String>| {
                    let mut cookie_header = hyper::header::Cookie::new();
                    for cookie_str in c {
                        let parsed = cookie::Cookie::parse(cookie_str.clone()).unwrap();
                        cookie_header.append(parsed.name().to_string(), parsed.value().to_string());
                    }
                    cookie_header
                });
            return cookies.unwrap();
        });
    return Box::new(response);
}

fn run_query(client: &SslClient, cookies: hyper::header::Cookie, from: &String, to: &String) -> IoFuture<MeazureResponse> {
    let uri = format!("https://surge.meazure.com/api/time-entry/?endDate={}&start=0&startDate={}&count=500", to, from).parse().unwrap();
    let mut req = hyper::Request::new(hyper::Method::Get, uri);
    req.headers_mut().set(cookies);
    let response = client.request(req)
        .and_then(move |res| { res.body().concat2() })
        .map(|chunk| {
            let r: MeazureResponse = serde_json::from_slice(&chunk).unwrap();
            return r;
        });
    return Box::new(response);
}

fn query_meazure(core: & mut Core, config: &Config, from: &String, to: &String) -> Result<Vec<Entry>, Box<Error>> {
    let handle = core.handle();
    let client = hyper::Client::configure()
        .connector(hyper_tls::HttpsConnector::new(4, &handle)?)
        .build(&handle);
    
    let tokens = get_request_tokens(&client);
    let cookies = tokens.and_then(|tokens| login(&client, &tokens, &config));
    let response = cookies.and_then(|cookies| run_query(&client, cookies, &from, &to));

    let parsed: MeazureResponse = core.run(response)?;
        
    let mut entries: Vec<Entry> = vec![];
    for se in parsed.data {
        entries.push(
            Entry {
                project: se.project_name,
                hours: se.duration_minutes as f64 / 60.0,
                date: se.bill_to_date
            }
        )
    }
    
    return Ok(entries);
}

fn merge_earnings(agg: &mut HashMap<String, Earnings>, entry: &Entry, rate: f32, key: &str) {
    let existing = match agg.get(key) {
        Some(e) => Earnings {hours: e.hours as f32 + entry.hours as f32, earnings: e.earnings + (entry.hours as f32 * rate)},
        None => Earnings {hours: entry.hours as f32, earnings: entry.hours as f32 * rate}
    };
    agg.insert(key.to_string(), existing);
}

fn aggregate_hours(entries: &Vec<Entry>, config: &Config) -> HashMap<String, Earnings> {
    let mut agg: HashMap<String, Earnings> = HashMap::new();
    for entry in entries.iter() {
        let proj = entry.project.as_str();
        let rate = match config.rates.get(proj) {
            Some(r) => *r,
            None => match config.rates.get("_default") {
                Some(r) => *r,
                None => 0.0
            }
        } as f32;
        merge_earnings(&mut agg, &entry, rate, proj);
        merge_earnings(&mut agg, &entry, rate, "total");
    }
    return agg;
}

fn make_projections(entries: &Vec<Entry>, totals: &Earnings, from: &String, to: &String) -> Projections {
    let local_now = Local::today().and_hms(0,0,0);
    let local_tomorrow = Local::today().succ().and_hms(23,59,59);
    let from_date = NaiveDate::parse_from_str(from.as_str(), DATE_FMT).unwrap();
    let to_date   = NaiveDate::parse_from_str(to.as_str(), DATE_FMT).unwrap();
    let local_from = Local.ymd(from_date.year(), from_date.month(), from_date.day());
    let local_to = Local.ymd(to_date.year(), to_date.month(), to_date.day());

    let today_entry = entries.iter().find(|&e| {
        let entry_date = Local.from_utc_date(
            &NaiveDate::parse_from_str(e.date.as_str(), DATE_FMT).unwrap()
        ).and_hms(0 ,0 ,0);
        let entry_timestamp = entry_date.timestamp();
        return entry_timestamp >= local_now.timestamp() && entry_timestamp <= local_tomorrow.timestamp();
    });

    let mut c = local_from.clone();
    let mut week_days = 0;
    let mut week_days_past = 0;
    let local_today;
    if today_entry.is_some() {
        local_today = Local::today().succ();
    } else {
        local_today = Local::today();
    };

    while c <= local_to {
        let day = c.weekday();
        let n = day.number_from_monday();
        if n < 6 {
            week_days += 1;
            
            if c < local_today {
                week_days_past += 1;
            }
        }
        c = c.succ();
    }

    let ratio_complete:f32 = week_days_past as f32/ week_days as f32;
    let percent_complete = (ratio_complete as f32 * 100.0).floor() as i64;
    let earnings_per_day = totals.earnings as f32 / week_days_past as f32;
    let estimated_earnings = earnings_per_day * week_days as f32;
    let estimated_hours = week_days as f32 * (totals.hours as f32 / week_days_past as f32);

    return Projections {
        week_days,
        week_days_past,
        percent_complete,
        avg_earnings_per_day: earnings_per_day,
        avg_hours_per_day: totals.hours as f32 / week_days_past as f32,
        estimated_earnings,
        estimated_hours
    };
}
