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

const CONFIG_FILE_NAME: &'static str = "meazure.config.json";
const DATE_FMT: &'static str = "%Y-%m-%d";

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

    println!("{}: {} - {}", config.uname, from_date, to_date);
    
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
    #[serde(rename = "Date")]
    date: i64,
    #[serde(rename = "DurationSeconds")]
    duration_seconds: i32,
    #[serde(rename = "Id")]
    id: i64,
    #[serde(rename = "Notes")]
    notes: String,
    #[serde(rename = "ProjectId")]
    project_id: i64,
    #[serde(rename = "ProjectName")]
    project_name: String,
    #[serde(rename = "TaskId")]
    task_id: i64,
    #[serde(rename = "TaskName")]
    task_name: String,
    #[serde(rename = "UserName")]
    user_name: String
}

struct Entry {
    project: String,
    hours: f64,
    date: i64
}

fn get_config(config_file_name: &str) -> io::Result<Config> {
    let mut f = try!(fs::File::open(config_file_name));
    let mut data = String::new();
    try!(f.read_to_string(&mut data));
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

fn query_meazure(core: & mut Core, config: &Config, from: &String, to: &String) -> Result<Vec<Entry>, Box<Error>> {
    let handle = core.handle();
    let client = hyper::Client::configure()
        .connector(hyper_tls::HttpsConnector::new(4, &handle)?)
        .build(&handle);
    
    let login_body = format!("{{\"Email\": \"{}\", \"Password\": \"{}\"}}", config.uname, config.pword);
    let login_uri = "https://meazure.surgeforward.com/Auth/Login".parse()?;
    let mut login_req = hyper::Request::new(hyper::Method::Post, login_uri);
    login_req.headers_mut().set(hyper::header::ContentType::json());
    login_req.headers_mut().set(hyper::header::ContentLength(login_body.len() as u64));
    login_req.set_body(login_body);

    let query_body = format!(r#"{{
    "ContentType": 1,
    "ReturnFields": [
      "Date",
      "DurationSeconds",
      "ProjectName",
      "TaskName"
    ],
    "ReturnFieldWidths": null,
    "Criteria": [
      {{
        "JoinOperator": "",
        "Field": "Date",
        "Operator": ">=",
        "Value": "{}"
      }},
      {{
        "JoinOperator": "and",
        "Field": "Date",
        "Operator": "<=",
        "Value": "{}"
      }}
    ],
    "Ordering": null}}"#, from, to);
    
    let work = client.request(login_req).and_then(|login_res| {
        let set_cookie = login_res.headers().get::<hyper::header::SetCookie>().unwrap();
        let cookie_s = &set_cookie[0];
        let cookie_parsed = cookie::Cookie::parse(cookie_s.to_string()).unwrap();
        let mut cookie_header = hyper::header::Cookie::new();
        cookie_header.set(cookie_parsed.name().to_string(), cookie_parsed.value().to_string());
        
        let uri = "https://meazure.surgeforward.com/Dashboard/RunQuery".parse().unwrap();
        let mut req = hyper::Request::new(hyper::Method::Post, uri);
        req.headers_mut().set(hyper::header::ContentType::json());
        req.headers_mut().set(hyper::header::ContentLength(query_body.len() as u64));
        req.headers_mut().set(cookie_header);
        req.set_body(query_body);
        client.request(req)
    }).and_then(move |res| { res.body().concat2() })
      .map(move |body| { 
          let v: Vec<MeazureEntry> = serde_json::from_slice(&body).unwrap();
          return v;
    });
    
    let parsed = core.run(work)?;
        
    let mut entries: Vec<Entry> = vec![];
    for se in parsed {
        entries.push(
            Entry {
                project: se.project_name,
                hours: se.duration_seconds as f64 / 60.0 / 60.0,
                date: se.date
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
        let date_time = Local.timestamp(e.date/1000, 0).timestamp() - local_now.offset().local_minus_utc() as i64;
        return date_time >= local_now.timestamp() && date_time <= local_tomorrow.timestamp();
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
    let percent_complete = (ratio_complete as f32 * 100 as f32).floor() as i64;
    let earnings_per_day = totals.earnings as f32 / week_days_past as f32;
    let estimated_earnings = earnings_per_day * week_days as f32;
    let estimated_hours = week_days as f32 * (totals.hours as f32 / week_days_past as f32);

    return Projections {
        week_days: week_days,
        week_days_past: week_days_past,
        percent_complete: percent_complete, 
        avg_earnings_per_day: earnings_per_day,
        avg_hours_per_day: totals.hours as f32 / week_days_past as f32,
        estimated_earnings: estimated_earnings,
        estimated_hours: estimated_hours
    };
}
