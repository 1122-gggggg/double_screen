#[cfg(any(target_os = "linux", windows))]
use std::path::Path;

use clap::{Parser, ValueEnum};
use serde::Serialize;

const NOT_MEASURED: &str = "NOT MEASURED";

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Format {
    Human,
    Json,
    Csv,
}

#[derive(Parser)]
#[command(
    name = "splitdesk-bench",
    about = "Synthetic capture/encode/loopback timings"
)]
struct Args {
    #[arg(long, value_enum, default_value_t = Format::Human)]
    format: Format,
}

#[derive(Serialize)]
struct BenchReport {
    capture: String,
    encode: String,
    decode: String,
    present: String,
    #[serde(rename = "input_rtt")]
    input_rtt: String,
}

impl BenchReport {
    fn not_measured() -> Self {
        Self {
            capture: NOT_MEASURED.into(),
            encode: NOT_MEASURED.into(),
            decode: NOT_MEASURED.into(),
            present: NOT_MEASURED.into(),
            input_rtt: NOT_MEASURED.into(),
        }
    }
}

fn gpu_encoder_present() -> bool {
    #[cfg(target_os = "linux")]
    {
        Path::new("/dev/nvidia0").exists()
    }
    #[cfg(windows)]
    {
        Path::new(r"C:\Windows\System32\nvEncodeAPI64.dll").exists()
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        false
    }
}

fn main() {
    let args = Args::parse();
    // No live NVENC/VAAPI/QSV encode is invoked here. GPU stages stay
    // NOT MEASURED unless a real encoder backend is wired and timed.
    let _gpu = gpu_encoder_present();
    let report = BenchReport::not_measured();
    match args.format {
        Format::Human => {
            println!("SplitDesk bench");
            println!("capture:    {}", report.capture);
            println!("encode:     {}", report.encode);
            println!("decode:     {}", report.decode);
            println!("present:    {}", report.present);
            println!("input RTT:  {}", report.input_rtt);
        }
        Format::Json => {
            println!("{}", serde_json::to_string_pretty(&report).unwrap());
        }
        Format::Csv => {
            println!("field,value");
            println!("capture,{}", report.capture);
            println!("encode,{}", report.encode);
            println!("decode,{}", report.decode);
            println!("present,{}", report.present);
            println!("input_rtt,{}", report.input_rtt);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parse_formats() {
        let human = Args::try_parse_from(["splitdesk-bench"]).unwrap();
        assert!(matches!(human.format, Format::Human));
        let json = Args::try_parse_from(["splitdesk-bench", "--format", "json"]).unwrap();
        assert!(matches!(json.format, Format::Json));
        let csv = Args::try_parse_from(["splitdesk-bench", "--format", "csv"]).unwrap();
        assert!(matches!(csv.format, Format::Csv));
    }
}
