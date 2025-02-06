use anyhow::Result;
use chrono::{Local, NaiveDateTime};
use ron::de::from_reader;
use ron::ser::{to_string_pretty, PrettyConfig};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::path::PathBuf;
use std::process::exit;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use tokio_modbus::prelude::*;

use clap::{Parser, Subcommand};

#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    pub poll_period: u32,
    pub devices: Vec<DeviceConfig>,
    pub database_path: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ModbusTcpConfig {
    pub ip: String,
    pub port: u16,
    pub tags: Vec<ModbusTag>,
}
#[derive(Debug, Deserialize, Serialize)]
pub struct ModbusRtuConfig {
    pub com: String,
    pub baudrate: u32,
    pub tags: Vec<ModbusTag>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ModbusTag {
    pub name: String,
    pub description: String,
    pub address: u16,
    pub value: ModbusValueType,
}

#[derive(Debug, Deserialize, Serialize)]
pub enum ModbusValueType {
    IntInput,
    RealInput,
    IntHolding,
    RealHolding,
    Coil,
}

#[derive(Debug, Deserialize, Serialize)]
pub enum DeviceType {
    ModbusTcp(ModbusTcpConfig),
    ModbusRtu(ModbusRtuConfig),
    OpcUa,
    EthernetIp,
    S7,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct DeviceConfig {
    pub name: String,
    pub device_type: DeviceType,
}

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    /// Optional name to operate on
    name: Option<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// does testing things
    Run {
        /// lists test values
        #[arg(short, long)]
        list: bool,
        /// Load config file
        #[arg(short, long, value_name = "FILE")]
        config: PathBuf,
    },
    CreateConfig,
}

fn main() {
    let cli = Cli::parse();

    if let Some(name) = cli.name.as_deref() {
        println!("Value for name: {name}");
    }

    match &cli.command {
        Some(Commands::Run { list, config }) => {
            // Process command and handle errors.
            process_run_cmd(list, config);
        }
        Some(Commands::CreateConfig) => {
            // Create a sample config for reference.
            process_create_config_cmd();
        }

        None => {}
    }

    loop {}
}

fn process_create_config_cmd() {
    let device_config = DeviceConfig {
        name: "PLC_2".to_string(),
        device_type: DeviceType::ModbusTcp(ModbusTcpConfig {
            ip: "192.168.0.1".to_string(),
            port: 5502,
            tags: vec![ModbusTag {
                name: "PIT-1001".to_string(),
                description: "Nothing".to_string(),
                address: 0,
                value: ModbusValueType::IntHolding,
            }],
        }),
    };

    let config = Config {
        poll_period: 60,
        devices: vec![device_config],
        database_path: "./db.sqlite".to_string(),
    };
    let pretty = PrettyConfig::new().depth_limit(4).enumerate_arrays(true);
    let s = to_string_pretty(&config, pretty).unwrap();
    println!("{}", &s);
}

fn process_run_cmd(list: &bool, config: &PathBuf) {
    if *list {
        println!("Printing testing lists...");
    } else {
        println!("Not printing testing lists...");
    }

    match parse_config_file(config) {
        Ok(config) => {
            println!("{} seconds", config.poll_period);
            println!("database: {}", &config.database_path);

            let conn = Connection::open(format!("{}", config.database_path));

            if let Ok(conn) = conn {
                init_db(&conn, &config);
                let db_ctx = Arc::new(Mutex::new(conn));
                for device in config.devices {
                    let mutex = db_ctx.clone();
                    match device.device_type {
                        DeviceType::ModbusTcp(modbus_config) => {
                            println!(
                                "Name: {}, Type: Modbus TCP, IP: {}",
                                device.name, modbus_config.ip
                            );

                            std::thread::spawn(move || {
                                poll_modbus_tcp(
                                    modbus_config,
                                    device.name,
                                    mutex,
                                    config.poll_period,
                                );
                            });
                        }
                        DeviceType::ModbusRtu(modbus_config) => {
                            println!(
                                "Name: {}, Type: Modbus TCP, COM: {}, Baudrate: {}",
                                device.name, modbus_config.com, modbus_config.baudrate
                            );

                            for tag in modbus_config.tags {
                                println!("{}, {}, {:?}", tag.name, tag.description, tag.value);
                            }
                        }
                        _ => {}
                    }
                }
            } else {
                println!("Could not open database.");
                exit(1);
            }
        }
        Err(e) => {
            println!("Error: {e}");
        }
    }
}

fn poll_modbus_tcp(
    modbus_config: ModbusTcpConfig,
    device_name: String,
    mutex: Arc<Mutex<Connection>>,
    poll_period: u32,
) {
    let socket_address_string = format!("{}:{}", modbus_config.ip, modbus_config.port);
    let socket_address = socket_address_string.parse();
    match socket_address {
        Ok(socket_address) => {
            loop {
                let ctx = tokio_modbus::client::sync::tcp::connect_with_timeout(
                    socket_address,
                    Some(Duration::from_secs(5)),
                );
                match ctx {
                    Ok(mut ctx) => {
                        println!("Connected to device: {}", &device_name);
                        loop {
                            if let Ok(conn) = mutex.lock() {
                                let now = Local::now().naive_local();
                                let timestamp = now.and_utc().timestamp();

                                for tag in modbus_config.tags.iter() {
                                    {
                                        let tag = tag;
                                        let ctx: &mut sync::Context = &mut ctx;
                                        let conn: &std::sync::MutexGuard<'_, Connection> = &conn;
                                        let device_name: &String = &device_name;
                                        match tag.value {
                                            ModbusValueType::IntHolding => {
                                                let buffer =
                                                    ctx.read_holding_registers(tag.address, 1);
                                                match buffer {
                                                    Ok(res) => {
                                                        conn.execute(
                                                    format!("INSERT INTO {} (id, timestamp, tag, description, value) VALUES (NULL, ?1, ?2, ?3, ?4)", &device_name).as_str(),
                                                    (device_name ,timestamp, &tag.name, &tag.description, res.unwrap()[0] as f32),
                                                )
                                                .unwrap();
                                                    }
                                                    Err(e) => {
                                                        println!(
                                                        "Failed to read tag: <{}> with address: <{}>. {}",
                                                        tag.name, tag.address, e
                                                    );
                                                        let ctx_retry = tokio_modbus::client::sync::tcp::connect_with_timeout(
                                                        socket_address,
                                                        Some(Duration::from_secs(5)),
                                                    );

                                                        if let Ok(ctx_retry) = ctx_retry {
                                                            *ctx = ctx_retry;
                                                            println!(
                                                                "Reconnected to Modbus server: {}",
                                                                &device_name
                                                            );
                                                        }
                                                    }
                                                }
                                            }
                                            ModbusValueType::RealHolding => {
                                                let buffer =
                                                    ctx.read_holding_registers(tag.address, 2);
                                                match buffer {
                                                    Ok(res) => {
                                                        let lsb = res.clone().unwrap()[0];
                                                        let msb = res.unwrap()[1];
                                                        conn.execute(
                                                    format!("INSERT INTO {} (id, timestamp, tag, description, value) VALUES (NULL, ?1, ?2, ?3, ?4)", &device_name).as_str(),
                                                    (timestamp, &tag.name, &tag.description, u16_to_float(lsb, msb)),
                                                )
                                                .unwrap();
                                                    }
                                                    Err(e) => {
                                                        println!(
                                                        "Failed to read tag: <{}> with address: <{}>. {}",
                                                        tag.name, tag.address, e
                                                    );
                                                        let ctx_retry = tokio_modbus::client::sync::tcp::connect_with_timeout(
                                                        socket_address,
                                                        Some(Duration::from_secs(5)),
                                                    );

                                                        if let Ok(ctx_retry) = ctx_retry {
                                                            *ctx = ctx_retry;
                                                            println!(
                                                                "Reconnected to Modbus server: {}",
                                                                &device_name
                                                            );
                                                        }
                                                    }
                                                }
                                            }
                                            _ => {}
                                        }
                                    };
                                }
                            }
                            std::thread::sleep(Duration::from_secs(poll_period as u64));
                        }
                    }
                    Err(e) => {
                        println!("Failed to connect to Modbus server. {e}");
                        //continue;
                    }
                }
                std::thread::sleep(Duration::from_secs(5));
            }
        }
        Err(e) => {
            println!("Failed to parse IP and PORT. {e}");
            exit(1);
        }
    }
}

fn init_db(conn: &Connection, config: &Config) {
    for device in config.devices.iter() {
        let name = &device.name;
        conn.execute(
            format!(
                "CREATE TABLE if not exists {} (
                            id INTEGER PRIMARY KEY,
                            timestamp INTEGER,
                            tag TEXT NOT NULL,
                            description TEXT NOT NULL,
                            value REAL
                        )",
                name
            )
            .as_str(),
            [],
        )
        .unwrap();
    }
}

pub fn parse_config_file(path: &PathBuf) -> Result<Config> {
    let file = File::open(path)?;
    let config: Config = from_reader(file)?;
    Ok(config)
}

fn u16_to_float(reg1: u16, reg2: u16) -> f32 {
    let data_32bit_rep = ((reg1 as u32) << 16) | reg2 as u32;
    let data_array = data_32bit_rep.to_ne_bytes();
    f32::from_ne_bytes(data_array)
}
