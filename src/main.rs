use actix_web::{web, App, HttpResponse, HttpServer};
use byteorder::ByteOrder;
use ctrlc;
use prometheus::Encoder;
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;

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

    println!("Connection to Bluez");
    let session = blurz::BluetoothSession::create_session(None).unwrap();
    let adapter = blurz::BluetoothAdapter::init(&session).unwrap();
    println!("Finding device");
    let device = adapter
        .get_device_list()
        .unwrap()
        .iter()
        .map(|path| blurz::BluetoothDevice::new(&session, path.clone()))
        .filter(|d| d.get_name().unwrap() == "iBBQ")
        .nth(0)
        .expect("Cannot find device.");
    println!("Connecting to device");
    device.connect(10_000).unwrap();
    println!("Finding services");
    let services: HashMap<_, _> = device
        .get_gatt_services()
        .unwrap()
        .iter()
        .map(|path| {
            let service = blurz::BluetoothGATTService::new(&session, path.clone());
            (service.get_uuid().unwrap(), service)
        })
        .collect();
    let characteristics: HashMap<_, _> = services
        .iter()
        .flat_map(|(_, service)| {
            service
                .get_gatt_characteristics()
                .unwrap()
                .iter()
                .map(|path| {
                    let characteristic =
                        blurz::BluetoothGATTCharacteristic::new(&session, path.clone());
                    (characteristic.get_uuid().unwrap(), characteristic)
                })
                .collect::<Vec<_>>()
                .into_iter()
        })
        .collect();

    println!("Authorising");
    // Authorise to device with magic data
    characteristics["0000fff2-0000-1000-8000-00805f9b34fb"]
        .write_value(
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
        )
        .unwrap();
    println!("Starting data collection");
    // Enable realtime data collection
    characteristics["0000fff5-0000-1000-8000-00805f9b34fb"]
        .write_value(vec![0x0B, 0x01, 0x00, 0x00, 0x00, 0x00], None)
        .unwrap();
    // Subscribe to realtime data collection
    if !characteristics["0000fff4-0000-1000-8000-00805f9b34fb"]
        .is_notifying()
        .unwrap()
    {
        characteristics["0000fff4-0000-1000-8000-00805f9b34fb"]
            .start_notify()
            .unwrap();
    }

    println!("Starting HTTP server");
    let running = Arc::new(AtomicBool::new(true));
    let running_server = running.clone();
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

        running_server.store(false, Ordering::Relaxed);
    });

    let running_interrupt = running.clone();
    ctrlc::set_handler(move || {
        running_interrupt.store(false, Ordering::Relaxed);
    }).expect("Error setting Ctrl-C handler");

    println!("Streaming data");
    while running.load(Ordering::Relaxed) {
        for event in session.incoming(1000).map(blurz::BluetoothEvent::from) {

            match event {
                Some(blurz::BluetoothEvent::Value { object_path, value }) => {
                    if object_path ==
                        characteristics["0000fff4-0000-1000-8000-00805f9b34fb"].get_id()
                    {
                        for probe in 0..value.len() / 2 {
                            let temp =
                                byteorder::LittleEndian::read_u16(&value[probe * 2..probe * 2 + 2]);
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
    characteristics["0000fff4-0000-1000-8000-00805f9b34fb"]
        .stop_notify()
        .unwrap();
    println!("Stop server");
    server.join().unwrap();
}
