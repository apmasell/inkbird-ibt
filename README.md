Inkbird IBT Temperature Probe Prometheus Exporter
=================================================

This program connects to an Inkbird iBBQ temperature sensor and converts the
temperature data into a format that can be streamed by Prometheus.

To build, install the Rust compiler and do:

     cargo build

Once build, you can run it:

     inkbird-ibt -b 127.0.0.1:9121

And point a Prometheus server to scrape it.

The program uses Bluez to collect data from the sensor and if the program is
stopped while streaming, Bluez can enter a state where it will not allow the
server to start streaming again. If this happens, restart Bluez, usually:

     sudo systemctl restart bluetooth
