use actix_web::{web, App, HttpResponse, HttpServer};
use byteorder::ByteOrder;
use ctrlc;
use prometheus::Encoder;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::thread;
macro_rules! wait_fail {
    ($res:expr, $interrupt_main:expr) => {
        match $res {
            Ok(val) => val,
            Err(e) => {
                println!("Bluetooth Error: {}; waiting.", e);
                (*$interrupt_main).1
                    .wait_timeout(
                        (*$interrupt_main).0.lock().unwrap(),
                        std::time::Duration::from_secs(60))
                    .unwrap();
                continue;
            }
        }
    };
}

macro_rules! wait_fail_option {
    ($res:expr, $interrupt_main:expr) => {
        match $res {
            Some(val) => val,
            None => {
                println!("Cannot find entry; waiting.");
                (*$interrupt_main).1
                    .wait_timeout(
                        (*$interrupt_main).0.lock().unwrap(),
                        std::time::Duration::from_secs(60))
                    .unwrap();
                continue;
                }
        }
    };
}

fn main() {
    let matches = clap::App::new("Inkbird Prometheus Exporter")
        .version("1.0")
        .author("Andre MAsella <andre@masella.name>")
        .about("Export Inkbird temperature data to Prometheus")
        .arg(
            clap::Arg::with_name("bind_address")
                .short("b")
                .long("bind")
                .value_name("ADDRESS")
                .help("Sets the address that the HTTP server should bind")
                .takes_value(true),
        )
        .get_matches();

    let registry = Arc::new(prometheus::Registry::new());
    let opts = prometheus::opts!("inkbird_ibt_temperature", "The temperature of the probes.");
    let gauge = prometheus::GaugeVec::new(opts, &vec!["probe"]).unwrap();
    registry.register(Box::new(gauge.clone())).unwrap();

    let interrupt_main = Arc::new((Mutex::new(true), Condvar::new()));
    let interrupt_interrupter = interrupt_main.clone();
    let interrupt_server = interrupt_main.clone();

    println!("Starting HTTP server");
    let server = thread::spawn(move || {
        HttpServer::new(move || {
            let metrics_registry = registry.clone();
            App::new().service(web::resource("/metrics").to(move || {

                let mut buffer = vec![];
                let encoder = prometheus::TextEncoder::new();
                let metric_families = metrics_registry.gather();
                encoder.encode(&metric_families, &mut buffer).unwrap();
                HttpResponse::Ok()
                    .content_type(encoder.format_type())
                    .body(buffer)
            }))
        }).bind(matches.value_of("bind_address").unwrap_or("127.0.0.1:9121"))
            .unwrap()
            .run()
            .unwrap();

        *interrupt_server.0.lock().unwrap() = false;
        interrupt_server.1.notify_one();
    });

    ctrlc::set_handler(move || {
        println!("Got interrupt. Shutting down...");
        *interrupt_interrupter.0.lock().unwrap() = false;
        interrupt_interrupter.1.notify_one();
    }).expect("Error setting Ctrl-C handler");
    while *(*interrupt_main).0.lock().unwrap() {
        println!("Connection to Bluez");
        let session = blurz::BluetoothSession::create_session(None).unwrap();
        let adapter = blurz::BluetoothAdapter::init(&session).unwrap();
        println!("Finding device");
        let device = wait_fail_option!(
            wait_fail!(adapter.get_device_list(), interrupt_main)
                .iter()
                .map(|path| blurz::BluetoothDevice::new(&session, path.clone()))
                .filter(|d| d.get_name().unwrap() == "iBBQ")
                .nth(0),
            interrupt_main
        );
        println!("Connecting to device");
        wait_fail!(device.connect(2_000), interrupt_main);
        println!("Finding services");
        let services: HashMap<_, _> = wait_fail!(device.get_gatt_services(), interrupt_main)
            .iter()
            .filter_map(|path| {
                let service = blurz::BluetoothGATTService::new(&session, path.clone());
                match service.get_uuid() {
                    Ok(uuid) => Some((uuid, service)),
                    Err(_) => None,
                }
            })
            .collect();
        let characteristics: HashMap<_, _> = services
            .iter()
            .flat_map(|(_, service)| {
                match service.get_gatt_characteristics() {
                    Ok(chars) => {
                        chars
                            .iter()
                            .filter_map(|path| {
                                let characteristic =
                                    blurz::BluetoothGATTCharacteristic::new(&session, path.clone());
                                match characteristic.get_uuid() {
                                    Ok(uuid) => Some((uuid, characteristic)),
                                    Err(_) => None,
                                }
                            })
                            .collect::<Vec<_>>()
                    }
                    Err(_) => vec![],
                }.into_iter()
            })
            .collect();

        println!("Authorising");
        // Authorise to device with magic data
        wait_fail!(
            wait_fail_option!(
                characteristics.get("0000fff2-0000-1000-8000-00805f9b34fb"),
                interrupt_main
            ).write_value(
                vec![
                    0x21,
                    0x07,
                    0x06,
                    0x05,
                    0x04,
                    0x03,
                    0x02,
                    0x01,
                    0xb8,
                    0x22,
                    0x00,
                    0x00,
                    0x00,
                    0x00,
                    0x00,
                ],
                None,
            ),
            interrupt_main
        );
        println!("Starting data collection");
        // Enable realtime data collection
        wait_fail!(
            wait_fail_option!(
                characteristics.get("0000fff5-0000-1000-8000-00805f9b34fb"),
                interrupt_main
            ).write_value(vec![0x0B, 0x01, 0x00, 0x00, 0x00, 0x00], None),
            interrupt_main
        );
        // Subscribe to realtime data collection
        if !wait_fail!(
            wait_fail_option!(
                characteristics.get("0000fff4-0000-1000-8000-00805f9b34fb"),
                interrupt_main
            ).is_notifying(),
            interrupt_main
        )
        {
            wait_fail!(
                characteristics["0000fff4-0000-1000-8000-00805f9b34fb"].start_notify(),
                interrupt_main
            );
        }


        println!("Streaming data");
        while *(*interrupt_main).0.lock().unwrap() {
            for event in session.incoming(1000).map(blurz::BluetoothEvent::from) {

                match event {
                    Some(blurz::BluetoothEvent::Value { object_path, value }) => {
                        if object_path ==
                            characteristics["0000fff4-0000-1000-8000-00805f9b34fb"].get_id()
                        {
                            for probe in 0..value.len() / 2 {
                                let temp = byteorder::LittleEndian::read_u16(
                                    &value[probe * 2..probe * 2 + 2],
                                );
                                gauge.with_label_values(&[&probe.to_string()]).set(
                                    if temp ==
                                        65526
                                    {
                                        std::f64::NAN
                                    } else {
                                        temp as f64 / 10.0
                                    },
                                );

                            }
                        }
                    }
                    _ => {}

                }
            }
        }
        println!("Disconnecting from device");
        wait_fail!(
            characteristics["0000fff4-0000-1000-8000-00805f9b34fb"].stop_notify(),
            interrupt_main
        );
    }
    println!("Stop server");
    server.join().unwrap();
}
