//! The speech models Settings → Voice offers, picked by the bench in `scripts/voice-bench/`
//! (coding prompts in five accents, with and without room noise, real read speech, Ukrainian and
//! Russian, and the user's own voice). Files are pinned to a repository commit, each with its
//! size and SHA-256.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    /// whisper.cpp on the GPU (Metal): one `.bin`.
    Whisper,
    /// NVIDIA's Parakeet TDT through sherpa-onnx, on the CPU.
    Parakeet,
    /// macOS's own speech recognition: nothing to download.
    Apple,
}

#[derive(Debug)]
pub struct File {
    /// Its name in the model's folder.
    pub name: &'static str,
    pub url: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}

#[derive(Debug)]
pub struct Model {
    /// Saved as the default in the `voice` setting: never rename one.
    pub id: &'static str,
    pub name: &'static str,
    /// One sentence: what it's good at, what it gives up, what it costs to run.
    pub description: &'static str,
    pub languages: &'static str,
    /// Who made it and its license, shown under it.
    pub credit: &'static str,
    pub engine: Engine,
    pub recommended: bool,
    pub files: &'static [File],
}

impl Model {
    pub fn bytes(&self) -> u64 {
        self.files.iter().map(|f| f.bytes).sum()
    }

    pub fn needs_download(&self) -> bool {
        !self.files.is_empty()
    }
}

macro_rules! whisper {
    ($file:literal) => {
        concat!("https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/", $file)
    };
}

macro_rules! parakeet {
    ($file:literal) => {
        concat!("https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8/resolve/84b478d541cdbf0c296a7ab5f1e20a6f8f998114/", $file)
    };
}

pub const MODELS: &[Model] = &[
    Model {
        id: "whisper-large-v3-turbo-q5",
        name: "Whisper Large v3 Turbo",
        description: "The most accurate here, in English and about 100 other languages, keeping code words like React or backend as written even in Ukrainian or Russian; about 1.5 s after you stop for 40 s of speech, on the GPU, and 0.8 GB of memory while loaded.",
        languages: "All languages",
        credit: "OpenAI Whisper · MIT",
        engine: Engine::Whisper,
        recommended: true,
        files: &[File {
            name: "ggml-large-v3-turbo-q5_0.bin",
            url: whisper!("ggml-large-v3-turbo-q5_0.bin"),
            bytes: 574_041_195,
            sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
        }],
    },
    Model {
        id: "parakeet-tdt-0.6b-v2",
        name: "Parakeet v2",
        description: "The fastest, and the best at natural English speech, about 1 s for 40 s of speech; English only, so other languages come out as English nonsense, and it holds 1.6 GB of memory while loaded.",
        languages: "English only",
        credit: "NVIDIA Parakeet TDT 0.6B v2 · CC BY 4.0",
        engine: Engine::Parakeet,
        recommended: false,
        files: &[
            File { name: "encoder.int8.onnx", url: parakeet!("encoder.int8.onnx"), bytes: 652_282_309, sha256: "92a10bd9c23936ea0566c29f963bc57c2a5df4261b6a8921c39abea66557ca8b" },
            File { name: "decoder.int8.onnx", url: parakeet!("decoder.int8.onnx"), bytes: 7_257_753, sha256: "4d097bae3bac5c96018d5293ba39f1b657296102b142affda40378f38b020f41" },
            File { name: "joiner.int8.onnx", url: parakeet!("joiner.int8.onnx"), bytes: 1_739_080, sha256: "77a29f3e5ac7d37074e0af465391b80f314015d5b97f0f90c6c7767f1d56f428" },
            File { name: "tokens.txt", url: parakeet!("tokens.txt"), bytes: 9_384, sha256: "ec182b70dd42113aff6c5372c75cac58c952443eb22322f57bbd7f53977d497d" },
        ],
    },
    Model {
        id: "whisper-base",
        name: "Whisper Base",
        description: "A small download that's light to run, well under a second and 0.4 GB of memory, but it gets about one word in seven wrong and misses many code words: for a quick try.",
        languages: "All languages",
        credit: "OpenAI Whisper · MIT",
        engine: Engine::Whisper,
        recommended: false,
        files: &[File {
            name: "ggml-base.bin",
            url: whisper!("ggml-base.bin"),
            bytes: 147_951_465,
            sha256: "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
        }],
    },
    Model {
        id: "apple",
        name: "Apple",
        description: "Built into macOS, so nothing to download and little memory, but weaker on code words and mixed languages, and macOS asks once to allow Speech Recognition.",
        languages: "Your Mac's languages",
        credit: "Built into macOS",
        engine: Engine::Apple,
        recommended: false,
        files: &[],
    },
];

pub fn find(id: &str) -> Option<&'static Model> {
    MODELS.iter().find(|m| m.id == id)
}
