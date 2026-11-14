//! `cargo run -p yh-voice --release --example transcribe <models folder> <model id> <audio>…`:
//! each file through a downloaded model, as the app does it, with timings. Downloads the model
//! first if `--download` is given.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let download = args.iter().any(|a| a == "--download");
    let args: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let [models, id, files @ ..] = args.as_slice() else {
        eprintln!("usage: transcribe <models folder> <model id> <audio>… [--download]");
        std::process::exit(2);
    };
    let models = std::path::PathBuf::from(models);
    let model = yh_voice::find(id).unwrap_or_else(|| panic!("no model {id}"));
    if download && !yh_voice::download::installed(&models, model) {
        let t = std::time::Instant::now();
        yh_voice::download::download(&models, model, &std::sync::atomic::AtomicBool::new(false)).expect("download");
        println!("downloaded {} in {:.0} s", model.id, t.elapsed().as_secs_f32());
    }
    let worker = yh_voice::Worker::start(models);
    for f in files {
        let audio = yh_voice::audio::read_file(std::path::Path::new(f.as_str())).expect("read");
        let t = worker.transcribe(model, audio, yh_voice::Hints::default()).recv().unwrap().expect("transcribe");
        println!("{f} ({:.1} s audio, load {:.2} s, took {:.2} s):\n  {}", t.audio_s, t.load_s, t.took_s, t.text);
    }
}
