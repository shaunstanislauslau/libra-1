// Copyright (c) The Diem Core Contributors
// SPDX-License-Identifier: Apache-2.0

//! The following is a slightly modified version of the file with the same name in the
//! rust-wallet crate. The original file may be found here:
//!
//! https://github.com/rust-bitcoin/rust-wallet/blob/master/wallet/src/mnemonic.rs

use crate::error::WalletError;
use anyhow::Result;
#[cfg(test)]
use diem_temppath::TempPath;
use mirai_annotations::*;
#[cfg(test)]
use rand::rngs::OsRng;
#[cfg(test)]
use rand::RngCore;
use sha2::{Digest, Sha256};

use std::{
    fs::{self, File},
    io::Write,
    path::Path,
};

/// Mnemonic seed for deterministic key derivation based on [BIP39](https://github.com/bitcoin/bips/blob/master/bip-0039.mediawiki).
/// The mnemonic must encode entropy in a multiple of 32 bits. With more entropy, security is
/// improved but the number of words increases.
///
/// The allowed sizes of entropy are 128, 160, 192, 224 and 256 bits as shown in the following
/// table.
///
/// +---------+-------+
/// | ENTROPY | WORDS |
/// +---------+-------+
/// |   128   |   12  |
/// |   160   |   15  |
/// |   192   |   18  |
/// |   224   |   21  |
/// |   256   |   24  |
/// +---------+-------+
pub struct Mnemonic(Vec<&'static str>);

impl ToString for Mnemonic {
    fn to_string(&self) -> String {
        self.0.as_slice().join(" ")
    }
}

impl Mnemonic {
    /// Generate mnemonic from string.
    pub fn from(s: &str) -> Result<Mnemonic> {
        let words: Vec<_> = s.split(' ').collect();
        let len = words.len();
        if len < 12 || len > 24 || len % 3 != 0 {
            return Err(WalletError::DiemWalletGeneric(
                "Mnemonic must have a word count of the following lengths: 24, 21, 18, 15, 12"
                    .to_string(),
            )
            .into());
        }

        let mut mnemonic = Vec::with_capacity(len);
        let mut bit_writer = U11BitWriter::new(len);
        for word in &words {
            if let Ok(idx) = WORDS.binary_search(word) {
                mnemonic.push(WORDS[idx]);
                bit_writer.write_u11(idx as u16);
            } else {
                return Err(WalletError::DiemWalletGeneric(
                    "Mnemonic contains an unknown word".to_string(),
                )
                .into());
            }
        }
        // Write any remaining bits.
        bit_writer.write_buffer();

        // This will never fail as we've already checked the word-list is not empty.
        let (checksum, entropy) = bit_writer
            .bytes
            .split_last()
            .unwrap_or_else(|| unreachable!());
        let computed_checksum = Sha256::digest(entropy)[0] >> (8 - len / 3);
        // Checksum validation.
        if *checksum != computed_checksum {
            return Err(
                WalletError::DiemWalletGeneric("Mnemonic checksum failed".to_string()).into(),
            );
        }
        Ok(Mnemonic(mnemonic))
    }

    /// Generate mnemonic from entropy byte-array.
    pub fn mnemonic(entropy: &[u8]) -> Result<Mnemonic> {
        let len = entropy.len();
        if len < 16 || len > 32 || len % 4 != 0 {
            return Err(WalletError::DiemWalletGeneric(
                "Entropy data for mnemonic must have one of the following byte lengths: \
                 32, 28, 24, 20, 16"
                    .to_string(),
            )
            .into());
        }

        // A checksum is generated by taking the first (entropy_size / 32) bits of entropy's SHA256
        // hash. Thus, for mnemonic purposes where maximum entropy is 256 bits, the checksum
        // requires 4 <= bits <= 8, so it always fits in one byte.
        let checksum = Sha256::digest(entropy)[0];

        let entropy_and_checksum = &[entropy, &[checksum]].concat()[..];

        let mut bit_reader = U11BitReader::new(entropy_and_checksum);

        let mnemonic_len = len * 3 / 4; // this is always divisible by 11.
        let mut mnemonic = Vec::with_capacity(mnemonic_len);
        for _ in 0..mnemonic_len {
            mnemonic.push(WORDS[bit_reader.read_u11() as usize]);
        }
        Ok(Mnemonic(mnemonic))
    }

    /// Write mnemonic to output_file_path.
    pub fn write(&self, output_file_path: &Path) -> Result<()> {
        if output_file_path.exists() && !output_file_path.is_file() {
            return Err(WalletError::DiemWalletGeneric(format!(
                "Output file {:?} for mnemonic backup is reserved",
                output_file_path.to_str(),
            ))
            .into());
        }
        let mut file = File::create(output_file_path)?;
        file.write_all(&self.to_string().as_bytes())?;
        Ok(())
    }

    /// Read mnemonic from input_file_path.
    pub fn read(input_file_path: &Path) -> Result<Self> {
        if input_file_path.exists() && input_file_path.is_file() {
            let mnemonic_string: String = fs::read_to_string(input_file_path)?;
            return Self::from(&mnemonic_string[..]);
        }
        Err(WalletError::DiemWalletGeneric(
            "Input file for mnemonic backup does not exist".to_string(),
        )
        .into())
    }
}

/// BitReader reads data from a byte slice at the granularity of 11 bits.
struct U11BitReader<'a> {
    bytes: &'a [u8],
    /// Position from the start of the slice, counted as bits instead of bytes.
    position: u16,
}

impl<'a> U11BitReader<'a> {
    /// Construct a new BitReader from a byte slice.
    fn new(bytes: &'a [u8]) -> U11BitReader<'a> {
        U11BitReader { bytes, position: 0 }
    }

    /// Read the value of 11 bits into a u16.
    fn read_u11(&mut self) -> u16 {
        let start_position = self.position;
        let end_position = start_position + 11;
        let mut value: u16 = 0;

        for i in start_position..end_position {
            let byte_index = (i / 8) as usize;
            let byte = self.bytes[byte_index];
            let shift = 7 - (i % 8);
            let bit = u16::from(byte >> shift) & 1;
            value = (value << 1) | bit;
        }

        self.position = end_position;
        value
    }
}

/// BitWriter writes data to a vector at the granularity of 11 bits.
struct U11BitWriter {
    bytes: Vec<u8>,
    unused: u16,
    buffer: u16,
    // invariant self.unused <= 8;
}

impl U11BitWriter {
    /// Create a new `BitWriter` around the given writer.
    fn new(mnemonic_len: usize) -> U11BitWriter {
        precondition!(mnemonic_len <= 24);
        U11BitWriter {
            bytes: Vec::with_capacity(11 * mnemonic_len / 8 + 1),
            unused: 8,
            buffer: 0,
        }
    }

    /// Write 11 bits.
    fn write_u11(&mut self, value: u16) {
        let mut nbits_remaining = 11;

        // Fill up a partial byte.
        if nbits_remaining >= self.unused && self.unused < 8 {
            let excess_bits = nbits_remaining - self.unused;
            self.buffer <<= self.unused;
            self.buffer |= (value >> excess_bits) & MASKS[self.unused as usize];

            self.bytes.push(self.buffer as u8);

            nbits_remaining = excess_bits;
            self.unused = 8;
            self.buffer = 0;
        }

        // Fill up full byte.
        while nbits_remaining >= 8 {
            nbits_remaining -= 8;
            self.bytes.push((value >> nbits_remaining) as u8);
        }
        verify!(nbits_remaining < 8);

        // Put the remaining bits in the buffer.
        if nbits_remaining > 0 {
            self.buffer <<= nbits_remaining;
            self.buffer |= value & MASKS[nbits_remaining as usize];
            self.unused -= nbits_remaining;
        }
    }

    fn write_buffer(&mut self) {
        if self.unused != 8 {
            self.bytes.push(self.buffer as u8);
        }
    }
}

/// Masks required for unsetting bits.
const MASKS: [u16; 8] = [0, 0b1, 0b11, 0b111, 0b1111, 0b11111, 0b11_1111, 0b111_1111];

// TODO: update this to hashmap or trie.
const WORDS: [&str; 2048] = [
    "abandon", "ability", "able", "about", "above", "absent", "absorb", "abstract", "absurd",
    "abuse", "access", "accident", "account", "accuse", "achieve", "acid", "acoustic", "acquire",
    "across", "act", "action", "actor", "actress", "actual", "adapt", "add", "addict", "address",
    "adjust", "admit", "adult", "advance", "advice", "aerobic", "affair", "afford", "afraid",
    "again", "age", "agent", "agree", "ahead", "aim", "air", "airport", "aisle", "alarm", "album",
    "alcohol", "alert", "alien", "all", "alley", "allow", "almost", "alone", "alpha", "already",
    "also", "alter", "always", "amateur", "amazing", "among", "amount", "amused", "analyst",
    "anchor", "ancient", "anger", "angle", "angry", "animal", "ankle", "announce", "annual",
    "another", "answer", "antenna", "antique", "anxiety", "any", "apart", "apology", "appear",
    "apple", "approve", "april", "arch", "arctic", "area", "arena", "argue", "arm", "armed",
    "armor", "army", "around", "arrange", "arrest", "arrive", "arrow", "art", "artefact", "artist",
    "artwork", "ask", "aspect", "assault", "asset", "assist", "assume", "asthma", "athlete",
    "atom", "attack", "attend", "attitude", "attract", "auction", "audit", "august", "aunt",
    "author", "auto", "autumn", "average", "avocado", "avoid", "awake", "aware", "away", "awesome",
    "awful", "awkward", "axis", "baby", "bachelor", "bacon", "badge", "bag", "balance", "balcony",
    "ball", "bamboo", "banana", "banner", "bar", "barely", "bargain", "barrel", "base", "basic",
    "basket", "battle", "beach", "bean", "beauty", "because", "become", "beef", "before", "begin",
    "behave", "behind", "believe", "below", "belt", "bench", "benefit", "best", "betray", "better",
    "between", "beyond", "bicycle", "bid", "bike", "bind", "biology", "bird", "birth", "bitter",
    "black", "blade", "blame", "blanket", "blast", "bleak", "bless", "blind", "blood", "blossom",
    "blouse", "blue", "blur", "blush", "board", "boat", "body", "boil", "bomb", "bone", "bonus",
    "book", "boost", "border", "boring", "borrow", "boss", "bottom", "bounce", "box", "boy",
    "bracket", "brain", "brand", "brass", "brave", "bread", "breeze", "brick", "bridge", "brief",
    "bright", "bring", "brisk", "broccoli", "broken", "bronze", "broom", "brother", "brown",
    "brush", "bubble", "buddy", "budget", "buffalo", "build", "bulb", "bulk", "bullet", "bundle",
    "bunker", "burden", "burger", "burst", "bus", "business", "busy", "butter", "buyer", "buzz",
    "cabbage", "cabin", "cable", "cactus", "cage", "cake", "call", "calm", "camera", "camp", "can",
    "canal", "cancel", "candy", "cannon", "canoe", "canvas", "canyon", "capable", "capital",
    "captain", "car", "carbon", "card", "cargo", "carpet", "carry", "cart", "case", "cash",
    "casino", "castle", "casual", "cat", "catalog", "catch", "category", "cattle", "caught",
    "cause", "caution", "cave", "ceiling", "celery", "cement", "census", "century", "cereal",
    "certain", "chair", "chalk", "champion", "change", "chaos", "chapter", "charge", "chase",
    "chat", "cheap", "check", "cheese", "chef", "cherry", "chest", "chicken", "chief", "child",
    "chimney", "choice", "choose", "chronic", "chuckle", "chunk", "churn", "cigar", "cinnamon",
    "circle", "citizen", "city", "civil", "claim", "clap", "clarify", "claw", "clay", "clean",
    "clerk", "clever", "click", "client", "cliff", "climb", "clinic", "clip", "clock", "clog",
    "close", "cloth", "cloud", "clown", "club", "clump", "cluster", "clutch", "coach", "coast",
    "coconut", "code", "coffee", "coil", "coin", "collect", "color", "column", "combine", "come",
    "comfort", "comic", "common", "company", "concert", "conduct", "confirm", "congress",
    "connect", "consider", "control", "convince", "cook", "cool", "copper", "copy", "coral",
    "core", "corn", "correct", "cost", "cotton", "couch", "country", "couple", "course", "cousin",
    "cover", "coyote", "crack", "cradle", "craft", "cram", "crane", "crash", "crater", "crawl",
    "crazy", "cream", "credit", "creek", "crew", "cricket", "crime", "crisp", "critic", "crop",
    "cross", "crouch", "crowd", "crucial", "cruel", "cruise", "crumble", "crunch", "crush", "cry",
    "crystal", "cube", "culture", "cup", "cupboard", "curious", "current", "curtain", "curve",
    "cushion", "custom", "cute", "cycle", "dad", "damage", "damp", "dance", "danger", "daring",
    "dash", "daughter", "dawn", "day", "deal", "debate", "debris", "decade", "december", "decide",
    "decline", "decorate", "decrease", "deer", "defense", "define", "defy", "degree", "delay",
    "deliver", "demand", "demise", "denial", "dentist", "deny", "depart", "depend", "deposit",
    "depth", "deputy", "derive", "describe", "desert", "design", "desk", "despair", "destroy",
    "detail", "detect", "develop", "device", "devote", "diagram", "dial", "diamond", "diary",
    "dice", "diesel", "diet", "differ", "digital", "dignity", "dilemma", "dinner", "dinosaur",
    "direct", "dirt", "disagree", "discover", "disease", "dish", "dismiss", "disorder", "display",
    "distance", "divert", "divide", "divorce", "dizzy", "doctor", "document", "dog", "doll",
    "dolphin", "domain", "donate", "donkey", "donor", "door", "dose", "double", "dove", "draft",
    "dragon", "drama", "drastic", "draw", "dream", "dress", "drift", "drill", "drink", "drip",
    "drive", "drop", "drum", "dry", "duck", "dumb", "dune", "during", "dust", "dutch", "duty",
    "dwarf", "dynamic", "eager", "eagle", "early", "earn", "earth", "easily", "east", "easy",
    "echo", "ecology", "economy", "edge", "edit", "educate", "effort", "egg", "eight", "either",
    "elbow", "elder", "electric", "elegant", "element", "elephant", "elevator", "elite", "else",
    "embark", "embody", "embrace", "emerge", "emotion", "employ", "empower", "empty", "enable",
    "enact", "end", "endless", "endorse", "enemy", "energy", "enforce", "engage", "engine",
    "enhance", "enjoy", "enlist", "enough", "enrich", "enroll", "ensure", "enter", "entire",
    "entry", "envelope", "episode", "equal", "equip", "era", "erase", "erode", "erosion", "error",
    "erupt", "escape", "essay", "essence", "estate", "eternal", "ethics", "evidence", "evil",
    "evoke", "evolve", "exact", "example", "excess", "exchange", "excite", "exclude", "excuse",
    "execute", "exercise", "exhaust", "exhibit", "exile", "exist", "exit", "exotic", "expand",
    "expect", "expire", "explain", "expose", "express", "extend", "extra", "eye", "eyebrow",
    "fabric", "face", "faculty", "fade", "faint", "faith", "fall", "false", "fame", "family",
    "famous", "fan", "fancy", "fantasy", "farm", "fashion", "fat", "fatal", "father", "fatigue",
    "fault", "favorite", "feature", "february", "federal", "fee", "feed", "feel", "female",
    "fence", "festival", "fetch", "fever", "few", "fiber", "fiction", "field", "figure", "file",
    "film", "filter", "final", "find", "fine", "finger", "finish", "fire", "firm", "first",
    "fiscal", "fish", "fit", "fitness", "fix", "flag", "flame", "flash", "flat", "flavor", "flee",
    "flight", "flip", "float", "flock", "floor", "flower", "fluid", "flush", "fly", "foam",
    "focus", "fog", "foil", "fold", "follow", "food", "foot", "force", "forest", "forget", "fork",
    "fortune", "forum", "forward", "fossil", "foster", "found", "fox", "fragile", "frame",
    "frequent", "fresh", "friend", "fringe", "frog", "front", "frost", "frown", "frozen", "fruit",
    "fuel", "fun", "funny", "furnace", "fury", "future", "gadget", "gain", "galaxy", "gallery",
    "game", "gap", "garage", "garbage", "garden", "garlic", "garment", "gas", "gasp", "gate",
    "gather", "gauge", "gaze", "general", "genius", "genre", "gentle", "genuine", "gesture",
    "ghost", "giant", "gift", "giggle", "ginger", "giraffe", "girl", "give", "glad", "glance",
    "glare", "glass", "glide", "glimpse", "globe", "gloom", "glory", "glove", "glow", "glue",
    "goat", "goddess", "gold", "good", "goose", "gorilla", "gospel", "gossip", "govern", "gown",
    "grab", "grace", "grain", "grant", "grape", "grass", "gravity", "great", "green", "grid",
    "grief", "grit", "grocery", "group", "grow", "grunt", "guard", "guess", "guide", "guilt",
    "guitar", "gun", "gym", "habit", "hair", "half", "hammer", "hamster", "hand", "happy",
    "harbor", "hard", "harsh", "harvest", "hat", "have", "hawk", "hazard", "head", "health",
    "heart", "heavy", "hedgehog", "height", "hello", "helmet", "help", "hen", "hero", "hidden",
    "high", "hill", "hint", "hip", "hire", "history", "hobby", "hockey", "hold", "hole", "holiday",
    "hollow", "home", "honey", "hood", "hope", "horn", "horror", "horse", "hospital", "host",
    "hotel", "hour", "hover", "hub", "huge", "human", "humble", "humor", "hundred", "hungry",
    "hunt", "hurdle", "hurry", "hurt", "husband", "hybrid", "ice", "icon", "idea", "identify",
    "idle", "ignore", "ill", "illegal", "illness", "image", "imitate", "immense", "immune",
    "impact", "impose", "improve", "impulse", "inch", "include", "income", "increase", "index",
    "indicate", "indoor", "industry", "infant", "inflict", "inform", "inhale", "inherit",
    "initial", "inject", "injury", "inmate", "inner", "innocent", "input", "inquiry", "insane",
    "insect", "inside", "inspire", "install", "intact", "interest", "into", "invest", "invite",
    "involve", "iron", "island", "isolate", "issue", "item", "ivory", "jacket", "jaguar", "jar",
    "jazz", "jealous", "jeans", "jelly", "jewel", "job", "join", "joke", "journey", "joy", "judge",
    "juice", "jump", "jungle", "junior", "junk", "just", "kangaroo", "keen", "keep", "ketchup",
    "key", "kick", "kid", "kidney", "kind", "kingdom", "kiss", "kit", "kitchen", "kite", "kitten",
    "kiwi", "knee", "knife", "knock", "know", "lab", "label", "labor", "ladder", "lady", "lake",
    "lamp", "language", "laptop", "large", "later", "latin", "laugh", "laundry", "lava", "law",
    "lawn", "lawsuit", "layer", "lazy", "leader", "leaf", "learn", "leave", "lecture", "left",
    "leg", "legal", "legend", "leisure", "lemon", "lend", "length", "lens", "leopard", "lesson",
    "letter", "level", "liar", "liberty", "diemry", "license", "life", "lift", "light", "like",
    "limb", "limit", "link", "lion", "liquid", "list", "little", "live", "lizard", "load", "loan",
    "lobster", "local", "lock", "logic", "lonely", "long", "loop", "lottery", "loud", "lounge",
    "love", "loyal", "lucky", "luggage", "lumber", "lunar", "lunch", "luxury", "lyrics", "machine",
    "mad", "magic", "magnet", "maid", "mail", "main", "major", "make", "mammal", "man", "manage",
    "mandate", "mango", "mansion", "manual", "maple", "marble", "march", "margin", "marine",
    "market", "marriage", "mask", "mass", "master", "match", "material", "math", "matrix",
    "matter", "maximum", "maze", "meadow", "mean", "measure", "meat", "mechanic", "medal", "media",
    "melody", "melt", "member", "memory", "mention", "menu", "mercy", "merge", "merit", "merry",
    "mesh", "message", "metal", "method", "middle", "midnight", "milk", "million", "mimic", "mind",
    "minimum", "minor", "minute", "miracle", "mirror", "misery", "miss", "mistake", "mix", "mixed",
    "mixture", "mobile", "model", "modify", "mom", "moment", "monitor", "monkey", "monster",
    "month", "moon", "moral", "more", "morning", "mosquito", "mother", "motion", "motor",
    "mountain", "mouse", "move", "movie", "much", "muffin", "mule", "multiply", "muscle", "museum",
    "mushroom", "music", "must", "mutual", "myself", "mystery", "myth", "naive", "name", "napkin",
    "narrow", "nasty", "nation", "nature", "near", "neck", "need", "negative", "neglect",
    "neither", "nephew", "nerve", "nest", "net", "network", "neutral", "never", "news", "next",
    "nice", "night", "noble", "noise", "nominee", "noodle", "normal", "north", "nose", "notable",
    "note", "nothing", "notice", "novel", "now", "nuclear", "number", "nurse", "nut", "oak",
    "obey", "object", "oblige", "obscure", "observe", "obtain", "obvious", "occur", "ocean",
    "october", "odor", "off", "offer", "office", "often", "oil", "okay", "old", "olive", "olympic",
    "omit", "once", "one", "onion", "online", "only", "open", "opera", "opinion", "oppose",
    "option", "orange", "orbit", "orchard", "order", "ordinary", "organ", "orient", "original",
    "orphan", "ostrich", "other", "outdoor", "outer", "output", "outside", "oval", "oven", "over",
    "own", "owner", "oxygen", "oyster", "ozone", "pact", "paddle", "page", "pair", "palace",
    "palm", "panda", "panel", "panic", "panther", "paper", "parade", "parent", "park", "parrot",
    "party", "pass", "patch", "path", "patient", "patrol", "pattern", "pause", "pave", "payment",
    "peace", "peanut", "pear", "peasant", "pelican", "pen", "penalty", "pencil", "people",
    "pepper", "perfect", "permit", "person", "pet", "phone", "photo", "phrase", "physical",
    "piano", "picnic", "picture", "piece", "pig", "pigeon", "pill", "pilot", "pink", "pioneer",
    "pipe", "pistol", "pitch", "pizza", "place", "planet", "plastic", "plate", "play", "please",
    "pledge", "pluck", "plug", "plunge", "poem", "poet", "point", "polar", "pole", "police",
    "pond", "pony", "pool", "popular", "portion", "position", "possible", "post", "potato",
    "pottery", "poverty", "powder", "power", "practice", "praise", "predict", "prefer", "prepare",
    "present", "pretty", "prevent", "price", "pride", "primary", "print", "priority", "prison",
    "private", "prize", "problem", "process", "produce", "profit", "program", "project", "promote",
    "proof", "property", "prosper", "protect", "proud", "provide", "public", "pudding", "pull",
    "pulp", "pulse", "pumpkin", "punch", "pupil", "puppy", "purchase", "purity", "purpose",
    "purse", "push", "put", "puzzle", "pyramid", "quality", "quantum", "quarter", "question",
    "quick", "quit", "quiz", "quote", "rabbit", "raccoon", "race", "rack", "radar", "radio",
    "rail", "rain", "raise", "rally", "ramp", "ranch", "random", "range", "rapid", "rare", "rate",
    "rather", "raven", "raw", "razor", "ready", "real", "reason", "rebel", "rebuild", "recall",
    "receive", "recipe", "record", "recycle", "reduce", "reflect", "reform", "refuse", "region",
    "regret", "regular", "reject", "relax", "release", "relief", "rely", "remain", "remember",
    "remind", "remove", "render", "renew", "rent", "reopen", "repair", "repeat", "replace",
    "report", "require", "rescue", "resemble", "resist", "resource", "response", "result",
    "retire", "retreat", "return", "reunion", "reveal", "review", "reward", "rhythm", "rib",
    "ribbon", "rice", "rich", "ride", "ridge", "rifle", "right", "rigid", "ring", "riot", "ripple",
    "risk", "ritual", "rival", "river", "road", "roast", "robot", "robust", "rocket", "romance",
    "roof", "rookie", "room", "rose", "rotate", "rough", "round", "route", "royal", "rubber",
    "rude", "rug", "rule", "run", "runway", "rural", "sad", "saddle", "sadness", "safe", "sail",
    "salad", "salmon", "salon", "salt", "salute", "same", "sample", "sand", "satisfy", "satoshi",
    "sauce", "sausage", "save", "say", "scale", "scan", "scare", "scatter", "scene", "scheme",
    "school", "science", "scissors", "scorpion", "scout", "scrap", "screen", "script", "scrub",
    "sea", "search", "season", "seat", "second", "secret", "section", "security", "seed", "seek",
    "segment", "select", "sell", "seminar", "senior", "sense", "sentence", "series", "service",
    "session", "settle", "setup", "seven", "shadow", "shaft", "shallow", "share", "shed", "shell",
    "sheriff", "shield", "shift", "shine", "ship", "shiver", "shock", "shoe", "shoot", "shop",
    "short", "shoulder", "shove", "shrimp", "shrug", "shuffle", "shy", "sibling", "sick", "side",
    "siege", "sight", "sign", "silent", "silk", "silly", "silver", "similar", "simple", "since",
    "sing", "siren", "sister", "situate", "six", "size", "skate", "sketch", "ski", "skill", "skin",
    "skirt", "skull", "slab", "slam", "sleep", "slender", "slice", "slide", "slight", "slim",
    "slogan", "slot", "slow", "slush", "small", "smart", "smile", "smoke", "smooth", "snack",
    "snake", "snap", "sniff", "snow", "soap", "soccer", "social", "sock", "soda", "soft", "solar",
    "soldier", "solid", "solution", "solve", "someone", "song", "soon", "sorry", "sort", "soul",
    "sound", "soup", "source", "south", "space", "spare", "spatial", "spawn", "speak", "special",
    "speed", "spell", "spend", "sphere", "spice", "spider", "spike", "spin", "spirit", "split",
    "spoil", "sponsor", "spoon", "sport", "spot", "spray", "spread", "spring", "spy", "square",
    "squeeze", "squirrel", "stable", "stadium", "staff", "stage", "stairs", "stamp", "stand",
    "start", "state", "stay", "steak", "steel", "stem", "step", "stereo", "stick", "still",
    "sting", "stock", "stomach", "stone", "stool", "story", "stove", "strategy", "street",
    "strike", "strong", "struggle", "student", "stuff", "stumble", "style", "subject", "submit",
    "subway", "success", "such", "sudden", "suffer", "sugar", "suggest", "suit", "summer", "sun",
    "sunny", "sunset", "super", "supply", "supreme", "sure", "surface", "surge", "surprise",
    "surround", "survey", "suspect", "sustain", "swallow", "swamp", "swap", "swarm", "swear",
    "sweet", "swift", "swim", "swing", "switch", "sword", "symbol", "symptom", "syrup", "system",
    "table", "tackle", "tag", "tail", "talent", "talk", "tank", "tape", "target", "task", "taste",
    "tattoo", "taxi", "teach", "team", "tell", "ten", "tenant", "tennis", "tent", "term", "test",
    "text", "thank", "that", "theme", "then", "theory", "there", "they", "thing", "this",
    "thought", "three", "thrive", "throw", "thumb", "thunder", "ticket", "tide", "tiger", "tilt",
    "timber", "time", "tiny", "tip", "tired", "tissue", "title", "toast", "tobacco", "today",
    "toddler", "toe", "together", "toilet", "token", "tomato", "tomorrow", "tone", "tongue",
    "tonight", "tool", "tooth", "top", "topic", "topple", "torch", "tornado", "tortoise", "toss",
    "total", "tourist", "toward", "tower", "town", "toy", "track", "trade", "traffic", "tragic",
    "train", "transfer", "trap", "trash", "travel", "tray", "treat", "tree", "trend", "trial",
    "tribe", "trick", "trigger", "trim", "trip", "trophy", "trouble", "truck", "true", "truly",
    "trumpet", "trust", "truth", "try", "tube", "tuition", "tumble", "tuna", "tunnel", "turkey",
    "turn", "turtle", "twelve", "twenty", "twice", "twin", "twist", "two", "type", "typical",
    "ugly", "umbrella", "unable", "unaware", "uncle", "uncover", "under", "undo", "unfair",
    "unfold", "unhappy", "uniform", "unique", "unit", "universe", "unknown", "unlock", "until",
    "unusual", "unveil", "update", "upgrade", "uphold", "upon", "upper", "upset", "urban", "urge",
    "usage", "use", "used", "useful", "useless", "usual", "utility", "vacant", "vacuum", "vague",
    "valid", "valley", "valve", "van", "vanish", "vapor", "various", "vast", "vault", "vehicle",
    "velvet", "vendor", "venture", "venue", "verb", "verify", "version", "very", "vessel",
    "veteran", "viable", "vibrant", "vicious", "victory", "video", "view", "village", "vintage",
    "violin", "virtual", "virus", "visa", "visit", "visual", "vital", "vivid", "vocal", "voice",
    "void", "volcano", "volume", "vote", "voyage", "wage", "wagon", "wait", "walk", "wall",
    "walnut", "want", "warfare", "warm", "warrior", "wash", "wasp", "waste", "water", "wave",
    "way", "wealth", "weapon", "wear", "weasel", "weather", "web", "wedding", "weekend", "weird",
    "welcome", "west", "wet", "whale", "what", "wheat", "wheel", "when", "where", "whip",
    "whisper", "wide", "width", "wife", "wild", "will", "win", "window", "wine", "wing", "wink",
    "winner", "winter", "wire", "wisdom", "wise", "wish", "witness", "wolf", "woman", "wonder",
    "wood", "wool", "word", "work", "world", "worry", "worth", "wrap", "wreck", "wrestle", "wrist",
    "write", "wrong", "yard", "year", "yellow", "you", "young", "youth", "zebra", "zero", "zone",
    "zoo",
];

#[test]
fn test_roundtrip_mnemonic() {
    let mut rng = OsRng;
    let mut buf = [0u8; 32];
    rng.fill_bytes(&mut buf[..]);
    let file = TempPath::new();
    let path = file.path();
    let mnemonic = Mnemonic::mnemonic(&buf[..]).unwrap();
    mnemonic.write(&path).unwrap();
    let other_mnemonic = Mnemonic::read(&path).unwrap();
    assert_eq!(mnemonic.to_string(), other_mnemonic.to_string());
}

#[test]
fn test_deterministic_mnemonic() {
    let zeros_entropy: [u8; 32] = [0; 32];
    let ones_entropy: [u8; 32] = [1; 32];

    let zeros_mnemonic = Mnemonic::mnemonic(&zeros_entropy).unwrap();
    let other_zeros_mnemonic = Mnemonic::mnemonic(&zeros_entropy).unwrap();
    let ones_mnemonic = Mnemonic::mnemonic(&ones_entropy).unwrap();

    let zeros_mnemonic_words = zeros_mnemonic.to_string();
    let other_zeros_mnemonic_words = other_zeros_mnemonic.to_string();
    let ones_mnemonic_words = ones_mnemonic.to_string();

    assert_eq!(zeros_mnemonic_words, other_zeros_mnemonic_words);
    assert_ne!(zeros_mnemonic_words, ones_mnemonic_words);
}

#[test]
fn test_entropy_length() {
    // entropy size in bytes.
    for size in (16..32).step_by(4) {
        let entropy = vec![0; size];
        let mnemonic = Mnemonic::mnemonic(&entropy);
        assert!(mnemonic.is_ok());
    }

    let some_invalid_entropy_sizes: [usize; 4] = [0, 8, 18, 36];
    for size in some_invalid_entropy_sizes.iter() {
        let entropy = vec![0; *size];
        let mnemonic = Mnemonic::mnemonic(&entropy);
        assert!(mnemonic.is_err());
    }
}

#[test]
fn test_entropy_to_word_number_compatibility() {
    for size in (16..32).step_by(4) {
        let entropy = vec![1; size];
        let mnemonic = Mnemonic::mnemonic(&entropy).unwrap();
        let mnemonic_string = mnemonic.to_string();
        let mnemonic_from_string = Mnemonic::from(&mnemonic_string[..]);
        assert!(mnemonic_from_string.is_ok());
    }
}

#[test]
fn test_bips39_vectors() {
    let tests = test_vectors_bip39();
    for t in tests.iter() {
        let entropy = hex::decode(t.seed).unwrap();
        let correct_mnemonic_string = t.mnemonic;
        let computed_mnemonic = Mnemonic::mnemonic(&entropy[..]).unwrap();
        let computed_mnemonic_string = computed_mnemonic.to_string();
        assert_eq!(correct_mnemonic_string, computed_mnemonic_string);
    }
}

#[test]
fn test_failed_checksum() {
    // CORRECT MNEMONIC: "abandon abandon abandon abandon abandon abandon abandon abandon abandon
    // abandon abandon about"

    // Test: change first word.
    let mut mnemonic = "science abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
    let mut computed_mnemonic = Mnemonic::from(&mnemonic[..]);
    assert!(computed_mnemonic.is_err());

    // Test: change last word.
    mnemonic = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon zoo";
    computed_mnemonic = Mnemonic::from(&mnemonic[..]);
    assert!(computed_mnemonic.is_err());

    // CORRECT MNEMONIC: "void come effort suffer camp survey warrior heavy shoot primary clutch
    // crush open amazing screen patrol group space point ten exist slush involve unfold"

    // Test: change second word.
    let mut mnemonic = "void black effort suffer camp survey warrior heavy shoot primary clutch crush open amazing screen patrol group space point ten exist slush involve unfold";
    let mut computed_mnemonic = Mnemonic::from(&mnemonic[..]);
    assert!(computed_mnemonic.is_err());

    // Test: change last word.
    mnemonic = "void come effort suffer camp survey warrior heavy shoot primary clutch crush open amazing screen patrol group space point ten exist slush involve holiday";
    computed_mnemonic = Mnemonic::from(&mnemonic[..]);
    assert!(computed_mnemonic.is_err());
}

/// Struct to handle BIP39 test vectors.
#[cfg(test)]
struct Test<'a> {
    seed: &'a str,
    mnemonic: &'a str,
}

/// Test vectors for BIP39 from https://github.com/trezor/python-mnemonic/blob/master/vectors.json
#[cfg(test)]
fn test_vectors_bip39<'a>() -> Vec<Test<'a>> {
    vec![
        Test {
            seed: "00000000000000000000000000000000",
            mnemonic: "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
        },
        Test {
            seed: "7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f",
            mnemonic: "legal winner thank year wave sausage worth useful legal winner thank yellow",
        },
        Test {
            seed: "80808080808080808080808080808080",
            mnemonic: "letter advice cage absurd amount doctor acoustic avoid letter advice cage above",
        },
        Test {
            seed: "ffffffffffffffffffffffffffffffff",
            mnemonic: "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong",
        },
        Test {
            seed: "000000000000000000000000000000000000000000000000",
            mnemonic: "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon agent",
        },
        Test {
            seed: "7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f",
            mnemonic: "legal winner thank year wave sausage worth useful legal winner thank year wave sausage worth useful legal will",
        },
        Test {
            seed: "808080808080808080808080808080808080808080808080",
            mnemonic: "letter advice cage absurd amount doctor acoustic avoid letter advice cage absurd amount doctor acoustic avoid letter always",
        },
        Test {
            seed: "ffffffffffffffffffffffffffffffffffffffffffffffff",
            mnemonic: "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo when",
        },
        Test {
            seed: "0000000000000000000000000000000000000000000000000000000000000000",
            mnemonic: "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art",
        },
        Test {
            seed: "7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f",
            mnemonic: "legal winner thank year wave sausage worth useful legal winner thank year wave sausage worth useful legal winner thank year wave sausage worth title",
        },
        Test {
            seed: "8080808080808080808080808080808080808080808080808080808080808080",
            mnemonic: "letter advice cage absurd amount doctor acoustic avoid letter advice cage absurd amount doctor acoustic avoid letter advice cage absurd amount doctor acoustic bless",
        },
        Test {
            seed: "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
            mnemonic: "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo vote",
        },
        Test {
            seed: "9e885d952ad362caeb4efe34a8e91bd2",
            mnemonic: "ozone drill grab fiber curtain grace pudding thank cruise elder eight picnic",
        },
        Test {
            seed: "6610b25967cdcca9d59875f5cb50b0ea75433311869e930b",
            mnemonic: "gravity machine north sort system female filter attitude volume fold club stay feature office ecology stable narrow fog",
        },
        Test {
            seed: "68a79eaca2324873eacc50cb9c6eca8cc68ea5d936f98787c60c7ebc74e6ce7c",
            mnemonic: "hamster diagram private dutch cause delay private meat slide toddler razor book happy fancy gospel tennis maple dilemma loan word shrug inflict delay length",
        },
        Test {
            seed: "c0ba5a8e914111210f2bd131f3d5e08d",
            mnemonic: "scheme spot photo card baby mountain device kick cradle pact join borrow",
        },
        Test {
            seed: "6d9be1ee6ebd27a258115aad99b7317b9c8d28b6d76431c3",
            mnemonic: "horn tenant knee talent sponsor spell gate clip pulse soap slush warm silver nephew swap uncle crack brave",
        },
        Test {
            seed: "9f6a2878b2520799a44ef18bc7df394e7061a224d2c33cd015b157d746869863",
            mnemonic: "panda eyebrow bullet gorilla call smoke muffin taste mesh discover soft ostrich alcohol speed nation flash devote level hobby quick inner drive ghost inside",
        },
        Test {
            seed: "23db8160a31d3e0dca3688ed941adbf3",
            mnemonic: "cat swing flag economy stadium alone churn speed unique patch report train",
        },
        Test {
            seed: "8197a4a47f0425faeaa69deebc05ca29c0a5b5cc76ceacc0",
            mnemonic: "light rule cinnamon wrap drastic word pride squirrel upgrade then income fatal apart sustain crack supply proud access",
        },
        Test {
            seed: "066dca1a2bb7e8a1db2832148ce9933eea0f3ac9548d793112d9a95c9407efad",
            mnemonic: "all hour make first leader extend hole alien behind guard gospel lava path output census museum junior mass reopen famous sing advance salt reform",
        },
        Test {
            seed: "f30f8c1da665478f49b001d94c5fc452",
            mnemonic: "vessel ladder alter error federal sibling chat ability sun glass valve picture",
        },
        Test {
            seed: "c10ec20dc3cd9f652c7fac2f1230f7a3c828389a14392f05",
            mnemonic: "scissors invite lock maple supreme raw rapid void congress muscle digital elegant little brisk hair mango congress clump",
        },
        Test {
            seed: "f585c11aec520db57dd353c69554b21a89b20fb0650966fa0a9d6f74fd989d8f",
            mnemonic: "void come effort suffer camp survey warrior heavy shoot primary clutch crush open amazing screen patrol group space point ten exist slush involve unfold",
        },
    ]
}
