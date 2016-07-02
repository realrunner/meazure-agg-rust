extern crate hyper;
extern crate rustc_serialize;
extern crate clap;
extern crate chrono;
extern crate rpassword;

use std::io::{self, Read, BufRead, Write};
use std::collections::HashMap;
use rustc_serialize::json;
use std::fs;
use std::result::Result;
use clap::{App};
use chrono::*;
use rpassword::read_password;

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

    let entries = query_meazure(&config, &from_date, &to_date).unwrap_or(vec!());
    if entries.len() < 1 {
        println!("No entires for that date range");
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
    println!("{}", json::as_pretty_json(&results));
}

#[derive(RustcDecodable, RustcEncodable)]
struct Config {
    uname: String,
    pword: String,
    rates: HashMap<String, i64>,
}

#[derive(RustcDecodable, RustcEncodable)]
struct Projections {
    week_days: i64,
    week_days_past: i64,
    percent_complete: i64, 
    avg_earnings_per_day: f32,
    avg_hours_per_day: f32,
    estimated_earnings: f32,
    estimated_hours: f32
}

#[derive(RustcDecodable, RustcEncodable)]
struct Earnings {
    hours: f32,
    earnings: f32
}

#[derive(RustcDecodable, RustcEncodable)]
struct Results {
    hours: HashMap<String, Earnings>,
    projections: Projections
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
    let config: Config = json::decode(&data).unwrap();
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

    let encoded = json::encode(&config).unwrap();
    let mut f = fs::File::create(config_file_name).unwrap();
    f.write(encoded.as_bytes()).unwrap();
    return config;
}

fn last_day_of_month(year: i32, month: u32) -> u32 {
    NaiveDate::from_ymd_opt(year, month + 1, 1).unwrap_or(NaiveDate::from_ymd(year + 1, 1, 1)).pred().day()
}

type EntryResult = Result<Vec<Entry>, String>;

fn query_meazure(config: &Config, from: &String, to: &String) -> EntryResult {
    let client = hyper::Client::new();
    let login_body = format!("{{\"Email\": \"{}\", \"Password\": \"{}\"}}", config.uname, config.pword);
    let login_res;
    match client.post("https://meazure.surgeforward.com/Auth/Login")
            .body(login_body.as_str())
            .send() {
        Result::Ok(r) => login_res = r,
        Result::Err(e) => return Err(e.to_string())
    };

    if login_res.status != hyper::Ok {
        return Result::Err("Failed logging in to meazure".to_string());
    }

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

    let mut jar = hyper::header::CookieJar::new(b"key");
    let set_cookie = login_res.headers.get::<hyper::header::SetCookie>().unwrap();
    set_cookie.apply_to_cookie_jar(&mut jar);
    let cookie = hyper::header::Cookie::from_cookie_jar(&jar);
    let query_req = client.post("https://meazure.surgeforward.com/Dashboard/RunQuery")
            .body(query_body.as_str())
            .header(cookie);

    let mut query_res;
    match query_req.send() {
        Result::Ok(r) => query_res = r,
        Result::Err(e) => return Err(e.to_string())
    };

    let mut results = String::new();
    query_res.read_to_string(&mut results).unwrap();

    let mut entries: Vec<Entry> = vec![];
    let parsed = json::Json::from_str(results.as_str()).unwrap();

    for e in parsed.as_array().unwrap().iter() {
        let o = e.as_object().unwrap();
        let project = o.get("ProjectName").unwrap().as_string().unwrap();
        let seconds = o.get("DurationSeconds").unwrap().as_f64().unwrap();
        let date = o.get("Date").unwrap().as_i64().unwrap();
        entries.push(Entry {project: project.to_string(), hours: seconds as f64 / 60.0 / 60.0, date: date});
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
                None => 0
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
        let date_time = Local.timestamp(e.date/1000, 0) - local_now.offset().local_minus_utc();
        return date_time >= local_now && date_time <= local_tomorrow;
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
