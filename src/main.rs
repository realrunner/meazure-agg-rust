extern crate hyper;
extern crate rustc_serialize;
extern crate clap;
extern crate chrono;

use std::io::Read;
use std::io;
use std::collections::HashMap;
use rustc_serialize::json;
use std::fs;
use std::result::Result;
use clap::{App};
use chrono::*;

const CONFIG_FILE_NAME: &'static str = "meazure.config.json";
const DATE_FMT: &'static str = "%Y-%m-%d";

fn main() {
    let msg = format!("-f, --from=[FROM] 'From date e.g. 2016-01-01 Defaults to the begging of the month'
                       -t, --to=[TO] 'To date e.g. 2016-01-31 Defaults to the end of the month'
                       -c, --config=[CONFIG] 'Config file. Defaults to {}", CONFIG_FILE_NAME);
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

    println!("{} - {}", from_date, to_date);



    match get_config(config_file_name) {
        Result::Ok(config) => print!("Uname {}", config.uname),
        Result::Err(e) => panic!("Error: {}", e)
    }

    let client = hyper::Client::new();
    match client.get("http://expedia.com").send() {
    	Result::Ok(mut res) => {
    		assert_eq!(res.status, hyper::Ok);
		    let mut s = String::new();
		    match res.read_to_string(&mut s) {
		    	Result::Ok(_) => print!("{}", s),
		    	Result::Err(e) => panic!("Error: {}", e)	
		    };
    	},
    	Result::Err(e) => panic!("Error: {}", e)
    };
}

#[derive(RustcDecodable, RustcEncodable)]
struct Config {
    uname: String,
    pword: String,
    rates: HashMap<String, i32>,
}

fn get_config(config_file_name: &str) -> io::Result<Config> {
    let mut f = try!(fs::File::open(config_file_name));
    let mut data = String::new();
    try!(f.read_to_string(&mut data));
    let config: Config = json::decode(&data).unwrap();
    Result::Ok(config)
}

fn last_day_of_month(year: i32, month: u32) -> u32 {
    NaiveDate::from_ymd_opt(year, month + 1, 1).unwrap_or(NaiveDate::from_ymd(year + 1, 1, 1)).pred().day()
}