use byteorder::ByteOrder;
use std::collections::HashMap;

fn main() {
    let session = blurz::BluetoothSession::create_session(None).unwrap();
    let adapter = blurz::BluetoothAdapter::init(&session).unwrap();
    let device = adapter
        .get_device_list()
        .unwrap()
        .iter()
        .map(|path| blurz::BluetoothDevice::new(&session, path.clone()))
        .filter(|d| d.get_name().unwrap() == "iBBQ")
        .nth(0)
        .expect("Cannot find device.");
    device.connect(10_000).unwrap();
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
    // Enable realtime data collection
    characteristics["0000fff5-0000-1000-8000-00805f9b34fb"]
        .write_value(vec![0x0B, 0x01, 0x00, 0x00, 0x00, 0x00], None)
        .unwrap();
    // Subscribe to realtime data collection
    characteristics["0000fff4-0000-1000-8000-00805f9b34fb"]
        .start_notify()
        .unwrap();

    loop {
        for event in session.incoming(1000).map(blurz::BluetoothEvent::from) {

            match event {
                Some(blurz::BluetoothEvent::Value { object_path, value }) => {
                    if object_path ==
                        characteristics["0000fff4-0000-1000-8000-00805f9b34fb"].get_id()
                    {
                        for probe in 0..value.len() / 2 {
                            let temp =
                                byteorder::LittleEndian::read_u16(&value[probe * 2..probe * 2 + 2]);
                            if temp != 65526 {
                                println!("{:?} {:?}", probe, temp as f32 / 10.0);
                            }

                        }
                    }
                }
                _ => {}

            }
        }
    }
}
