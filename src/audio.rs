
use cpal::traits::{HostTrait, DeviceTrait, StreamTrait};

use std::sync::{Arc, Mutex};
use std::sync::mpsc;

#[allow(dead_code)]
enum AudioChannelMessage {
	SendNext(i32),
	NOP
}


enum SourceType {
	FLAC(String),
	SOURCELESS,
	UNSUPPORTED
}
impl SourceType {
	fn from_local(file: String) -> SourceType {
		let mut split = file.split(".");
		// This doesn't support non-trivial file names. (e.g., untitled.new.flac which would be
		// processed as (untitled, new) instead of (untitled.new, flac) as a
		// (filename, extension) tuple)
		// Discard filename
		split.next().expect("empty string provided for audio source.").to_string();
		// Use extension
		let file_extension = split.next().expect("source audio file has no file extension.").to_string();

		match file_extension.to_lowercase().as_str() {
			"flac" => SourceType::FLAC(file),
			"" => SourceType::SOURCELESS,
			_ => SourceType::UNSUPPORTED
		}
	}

	fn from_stream() -> SourceType {
		SourceType::UNSUPPORTED
	}
}

struct PhyiscalAudioDevice {
	host: cpal::Host,
	device: cpal::Device,
	supported_configs_range: cpal::SupportedOutputConfigs,
	sample_format: cpal::SampleFormat,
	config: cpal::StreamConfig,
	stream: Option<cpal::Stream>,
}
enum AudioDevice {
	PHYSICAL(PhyiscalAudioDevice),
	VIRUTAL,
	NONE
}
/* Sink Trait: A trait intended for a Consumer that discards the passed audio.
 */
trait AudioSink {
	fn new(rx_channel: mpsc::Receiver<f32>) -> Self;
	fn connect(&mut self) -> ();
}
struct AudioConsumer {
	data_channel: Arc<Mutex<mpsc::Receiver<f32>>>,
	audio_device: AudioDevice
}

fn err_fn<T>(err: T) where T: std::fmt::Display {
	eprintln!("an error occurred on the output audio stream: {}", err);
}

impl AudioConsumer {
	fn new(data_channel: mpsc::Receiver<f32>) -> AudioConsumer {
		let host = cpal::default_host();

		let device = host.default_output_device()
			.expect("No default audio device.");
		let mut supported_configs_range = device.supported_output_configs()
			.expect("Error querying devices.");
		let supported_config = supported_configs_range.next()
			.expect("No supported stream configuration.")
			.with_max_sample_rate();

		let mut ac = AudioConsumer {
			data_channel: Arc::new(Mutex::new(data_channel)),
			audio_device: AudioDevice::PHYSICAL(PhyiscalAudioDevice {
				host,
				device,
				supported_configs_range,
				sample_format: supported_config.sample_format(),
				config: supported_config.into(),
				stream: None
			}),
		};
		ac.connect();
		ac
	}

	fn connect(&mut self) -> () {
		match &mut self.audio_device {
			AudioDevice::PHYSICAL(physical_device) => {
				let data_channel_arc = Arc::clone( & self.data_channel);

				let clu = move | data: & mut [f32],
								 _: & cpal::OutputCallbackInfo | {
					let lock = match ( * data_channel_arc).lock() {
						Ok(lock) => lock,
						Err(_) => panic ! ("other thread panicked") // other thread panicked
					};

					let data_channel = & * lock;

					for sample in data.iter_mut() {
						let s = data_channel.recv_timeout(std::time::Duration::from_millis(1)).unwrap_or(0.0f32);
						//println!("{}", s);
						* sample = cpal::Sample::from( & s);
					}
				};

				physical_device.stream = Option::Some( match physical_device.sample_format {
					cpal::SampleFormat::F32 => physical_device.device.build_output_stream( &physical_device.config, clu, err_fn),
					cpal::SampleFormat::I16 => physical_device.device.build_output_stream( &physical_device.config, clu, err_fn),
					cpal::SampleFormat::U16 => physical_device.device.build_output_stream( &physical_device.config, clu, err_fn),
				}.expect("Stream died (ooof)."));

				physical_device.stream.as_ref().unwrap().play().unwrap();
			},
			_ => {}
		}
	}
}

impl AudioSink for AudioConsumer {
	fn new(rx_channel: mpsc::Receiver<f32>) -> Self {
		let mut ac = AudioConsumer {
			data_channel: Arc::new(Mutex::new(rx_channel)),
			audio_device: AudioDevice::NONE
		};
		ac.connect();
		ac
	}

	fn connect(&mut self) -> () {

	}
}

/*
 * AudioProducer: Representation of object responsible for producing audio to a given internal audio channel.
 */
struct AudioProducer {
	data_channel: Arc<Mutex<mpsc::Sender<f32>>>,
	source_type: Arc<SourceType>,
	thread: Option<std::thread::JoinHandle<()>>,
}

/* LocalAudioProducer: Representation of an AudioProducer that gets its audio data from a file.
 */
trait LocalAudioProducer {
	fn new(_: String, _: mpsc::Sender<f32>) -> AudioProducer;

	fn connect(&mut self) -> ();
}

/* StreamAudioProducer: Representation of a AudioProducer that gets its audio data from a network stream.
 */
trait StreamAudioProducer {
	fn new() -> AudioProducer;
}

/* DeviceAudioProducer: Representation of a AudioProducer that gets its audio data from a device on the system.
 */
trait DeviceAudioProducer {
	fn new() -> AudioProducer;
}

impl LocalAudioProducer for AudioProducer {
	fn new(file: String, data_channel: mpsc::Sender<f32>) -> AudioProducer {
		let mut ap = AudioProducer {
			data_channel: Arc::new(Mutex::new(data_channel)),
			source_type: Arc::new(SourceType::from_local(file)),
			thread: None,
		};
		ap.connect();
		ap
	}

	fn connect(&mut self) -> () {
		// Grab a shared access to data_channel and source_type to use in the thread.
		let tx_channel = Arc::clone(&self.data_channel);
		let source_type = Arc::clone(&self.source_type);

		let source_type_2 = Arc::clone(&self.source_type);

		self.thread = match &*source_type {
			SourceType::FLAC(_) => Some(std::thread::spawn(move || {
				let flac_file = match &*source_type_2 { SourceType::FLAC(flac_file) => flac_file, _ => panic!("unreachable") };

				let mut reader = claxon::FlacReader::open(flac_file).expect("no file.");
				let samples = reader.samples();

				let lock = match (*tx_channel).lock() {
					Ok(lock) => lock,
					Err(_) => panic!("other thread panicked") // other thread panicked
				};

				let data_channel = &*lock;

				for sample in samples {
					let s = (sample.unwrap_or(0) as f32) / (std::i32::MAX as f32) * 160.0;
					data_channel.send(s).unwrap();
				}
			})),
			SourceType::SOURCELESS => None,
			SourceType::UNSUPPORTED => None
		};
	}
}
/*
impl StreamAudioProducer for AudioProducer {
    fn new() -> AudioProducer {
        AudioProducer {
            data_channel: Arc::<mpsc::Sender<f32>>::new_uninit(),
            source_type: std::sync::Arc::new(SourceType::SOURCELESS),
            thread: None
        }
    }
}*/

impl AudioProducer {

}


/* AudioCable - Digital representation of a physical connection between a source and a destination.

 */
pub struct AudioCable {
	data_source: AudioProducer,
	data_destination: AudioConsumer,
}

impl AudioCable {
	pub fn new(audio_source: String) -> Self {
		let (tx, rx): (std::sync::mpsc::Sender<f32>, std::sync::mpsc::Receiver<f32>) = std::sync::mpsc::channel();

		let comm_chan = crossbeam_channel::unbounded::<i32>();

		AudioCable {
			data_source: <AudioProducer as LocalAudioProducer>::new(audio_source, tx),
			data_destination: AudioConsumer::new(rx),
		}
	}
}