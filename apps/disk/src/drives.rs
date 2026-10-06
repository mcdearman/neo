//! Drives and what can be done to them: mounting, unmounting, ejecting and
//! formatting. The work is done by the system's own tools; this module
//! reads what they report and builds the commands, and is strict about
//! which drives may be formatted at all.

use std::path::PathBuf;

/// A volume: a formatted partition, or a whole drive with nothing on it yet.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Drive {
    /// What the system calls it: `disk4s1` on macOS, `/dev/sdb1` on Linux.
    pub id: String,
    /// The whole drive it is part of, for ejecting.
    pub parent: String,
    /// Its label, if it has one.
    pub name: String,
    pub size: u64,
    /// The file system on it, as the system names it. Empty if none.
    pub fs: String,
    pub mount: Option<PathBuf>,
    /// Plugged in from outside, or removable media.
    pub external: bool,
    /// The disk this computer started from, or part of it.
    pub startup: bool,
    /// A whole drive rather than a partition of one.
    pub whole: bool,
}

impl Drive {
    /// The name to show, and to type when confirming a format.
    pub fn title(&self) -> &str {
        if self.name.is_empty() { &self.id } else { &self.name }
    }
}

/// A file system a drive can be formatted with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fs {
    ExFat,
    Fat32,
    Apfs,
    MacJournaled,
    Ext4,
}

impl Fs {
    /// The choices on this system, the most widely readable first.
    pub fn available() -> &'static [Fs] {
        if cfg!(target_os = "macos") {
            &[Fs::ExFat, Fs::Fat32, Fs::Apfs, Fs::MacJournaled]
        } else if cfg!(windows) {
            &[]
        } else {
            &[Fs::ExFat, Fs::Fat32, Fs::Ext4]
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Fs::ExFat => "exFAT",
            Fs::Fat32 => "FAT32",
            Fs::Apfs => "APFS",
            Fs::MacJournaled => "Mac OS Extended",
            Fs::Ext4 => "ext4",
        }
    }

    /// Who can read a drive formatted this way.
    pub fn note(self) -> &'static str {
        match self {
            Fs::ExFat => "Works with macOS, Windows and Linux. The usual choice for a drive that moves between computers.",
            Fs::Fat32 => "Works with nearly everything, including cameras and consoles, but no file can be over 4 GB.",
            Fs::Apfs => "For Macs only, and the best choice for a drive that stays with them.",
            Fs::MacJournaled => "For Macs, including old ones that cannot read APFS.",
            Fs::Ext4 => "For Linux. Other systems need extra software to read it.",
        }
    }

    /// Checks a name for a drive formatted this way, and returns it as it
    /// will be written.
    pub fn label(self, name: &str) -> Result<String, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("Give the drive a name.".into());
        }
        if name.chars().any(|c| c.is_control() || "/\\:*?\"<>|".contains(c)) {
            return Err("A drive's name can't contain / \\ : * ? \" < > or |.".into());
        }
        let (most, what) = match self {
            Fs::Fat32 => (11, "FAT32"),
            Fs::ExFat => (15, "exFAT"),
            Fs::Ext4 => (16, "ext4"),
            Fs::Apfs | Fs::MacJournaled => (255, "this file system"),
        };
        if name.chars().count() > most || (self == Fs::Ext4 && name.len() > most) {
            return Err(format!("Names on {what} can be {most} characters at most."));
        }
        if self == Fs::Fat32 {
            if !name.is_ascii() {
                return Err("Names on FAT32 use plain letters, digits and spaces.".into());
            }
            return Ok(name.to_ascii_uppercase());
        }
        Ok(name.to_owned())
    }
}

/// Something to do to a drive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Mount,
    Unmount,
    /// Unmount everything on the drive so it can be unplugged.
    Eject,
    /// Erase it and make a new, empty file system.
    Format {
        fs: Fs,
        name: String,
    },
}

/// Why a drive may not be formatted from here, if it may not.
///
/// NeoDisk formats external drives only. The disk the computer started
/// from, and any other built into it, are refused outright: a mistake
/// there loses the system, and there are better tools for a job that big.
pub fn format_refusal(d: &Drive) -> Option<&'static str> {
    if d.startup {
        Some("This is the disk your computer started from. It can't be formatted here.")
    } else if !d.external {
        Some("This drive is built into the computer. NeoDisk only formats external drives.")
    } else {
        None
    }
}

/// Whether an identifier is the plain kind the system gives out, so that
/// nothing else is ever handed to a command.
fn plain_id(id: &str) -> bool {
    if cfg!(target_os = "macos") {
        let rest = id.strip_prefix("disk").unwrap_or("");
        !rest.is_empty() && rest.starts_with(|c: char| c.is_ascii_digit()) && rest.chars().all(|c| c.is_ascii_digit() || c == 's')
    } else {
        id.strip_prefix("/dev/").is_some_and(|r| !r.is_empty() && r.chars().all(|c| c.is_ascii_alphanumeric() || "_-/".contains(c)) && !r.contains(".."))
    }
}

/// The commands that carry out `action` on `d`, in order, each as a
/// program and its arguments. Refuses a format that is not allowed.
pub fn commands(d: &Drive, action: &Action) -> Result<Vec<Vec<String>>, String> {
    if !plain_id(&d.id) || (*action == Action::Eject && !plain_id(&d.parent)) {
        return Err(format!("\"{}\" is not a drive NeoDisk recognises.", d.id));
    }
    let argv = |words: &[&str]| words.iter().map(|w| (*w).to_owned()).collect::<Vec<_>>();
    if cfg!(windows) {
        return Err("Mounting and formatting are not available on Windows yet.".into());
    }
    let mac = cfg!(target_os = "macos");
    Ok(match action {
        Action::Mount if mac => vec![argv(&["diskutil", "mount", &d.id])],
        Action::Mount => vec![argv(&["udisksctl", "mount", "-b", &d.id])],
        Action::Unmount if mac => vec![argv(&["diskutil", "unmount", &d.id])],
        Action::Unmount => vec![argv(&["udisksctl", "unmount", "-b", &d.id])],
        Action::Eject if mac => vec![argv(&["diskutil", "eject", &d.parent])],
        Action::Eject => {
            let mut steps = vec![];
            if d.mount.is_some() {
                steps.push(argv(&["udisksctl", "unmount", "-b", &d.id]));
            }
            steps.push(argv(&["udisksctl", "power-off", "-b", &d.parent]));
            steps
        }
        Action::Format { fs, name } => {
            if let Some(why) = format_refusal(d) {
                return Err(why.into());
            }
            if !Fs::available().contains(fs) {
                return Err(format!("{} is not available on this system.", fs.name()));
            }
            let name = fs.label(name)?;
            if mac {
                let personality = match fs {
                    Fs::ExFat => "ExFAT",
                    Fs::Fat32 => "MS-DOS FAT32",
                    Fs::Apfs => "APFS",
                    Fs::MacJournaled => "Journaled HFS+",
                    Fs::Ext4 => unreachable!("not offered on macOS"),
                };
                // A whole drive is given a fresh layout; a volume is remade in place.
                vec![argv(&["diskutil", if d.whole { "eraseDisk" } else { "eraseVolume" }, personality, &name, &d.id])]
            } else {
                let mut steps = vec![];
                if d.mount.is_some() {
                    steps.push(argv(&["udisksctl", "unmount", "-b", &d.id]));
                }
                // Making a file system needs the administrator's say-so.
                steps.push(match fs {
                    Fs::ExFat => argv(&["pkexec", "mkfs.exfat", "-n", &name, &d.id]),
                    Fs::Fat32 => argv(&["pkexec", "mkfs.vfat", "-F", "32", "-n", &name, &d.id]),
                    Fs::Ext4 => argv(&["pkexec", "mkfs.ext4", "-F", "-L", &name, &d.id]),
                    Fs::Apfs | Fs::MacJournaled => unreachable!("not offered on Linux"),
                });
                steps
            }
        }
    })
}

/// Runs commands in order, stopping at the first that fails. Returns what
/// the last one said, or what the failing one said.
pub fn run(steps: &[Vec<String>]) -> Result<String, String> {
    let mut said = String::new();
    for step in steps {
        let Some((program, args)) = step.split_first() else {
            continue;
        };
        let out = std::process::Command::new(program).args(args).stdin(std::process::Stdio::null()).output().map_err(|e| format!("Could not run {program}: {e}"))?;
        let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).trim().to_owned();
        if !out.status.success() {
            let why = [text(&out.stderr), text(&out.stdout)].into_iter().find(|s| !s.is_empty()).unwrap_or_else(|| format!("{program} failed"));
            return Err(why);
        }
        said = text(&out.stdout);
    }
    Ok(said)
}

/// Every drive worth showing: the startup disk, and whatever is plugged in.
pub fn list() -> Result<Vec<Drive>, String> {
    let read = |program: &str, args: &[&str]| -> Result<String, String> {
        let out = std::process::Command::new(program).args(args).output().map_err(|e| format!("Listing drives needs {program}: {e}"))?;
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    };
    if cfg!(target_os = "macos") {
        Ok(parse_diskutil(&read("diskutil", &["info", "-all"])?))
    } else if cfg!(windows) {
        Err("Listing drives is not available on Windows yet.".into())
    } else {
        parse_lsblk(&read("lsblk", &["-J", "-b", "-o", "NAME,PATH,SIZE,FSTYPE,LABEL,MOUNTPOINT,RM,HOTPLUG,TYPE"])?)
    }
}

/// Reads `diskutil info -all`: a block of `Key: value` lines per device,
/// with a row of asterisks between blocks.
pub fn parse_diskutil(text: &str) -> Vec<Drive> {
    let mut all = vec![];
    for block in text.split("**********") {
        let field = |key: &str| block.lines().find_map(|l| l.trim().strip_prefix(key)?.strip_prefix(':').map(|v| v.trim().to_owned())).filter(|v| !v.is_empty());
        let Some(id) = field("Device Identifier") else {
            continue;
        };
        let has_fs = |v: &String| !v.starts_with("Not applicable") && v != "None";
        let fs = field("File System Personality").filter(has_fs).unwrap_or_default();
        let name = field("Volume Name").filter(has_fs).unwrap_or_default();
        let mount = field("Mount Point").filter(has_fs).map(PathBuf::from);
        // "500.3 GB (500277792768 Bytes) (exactly ...)".
        let size = field("Disk Size").and_then(|v| v.split('(').nth(1)?.split_whitespace().next()?.parse().ok()).unwrap_or(0);
        let external = field("Device Location").is_some_and(|v| v == "External") || field("Removable Media").is_some_and(|v| v == "Removable");
        let whole = field("Whole").is_some_and(|v| v == "Yes");
        all.push(Drive { parent: field("Part of Whole").unwrap_or_else(|| id.clone()), startup: mount.as_deref() == Some(std::path::Path::new("/")), id, name, size, fs, mount, external, whole });
    }
    // Shown: the startup volume; every external volume with a file system,
    // bar the small ones a partition scheme keeps for itself; and external
    // drives with nothing on them yet, which have no volume to show.
    let formatted: Vec<String> = all.iter().filter(|d| !d.whole && !d.fs.is_empty()).map(|d| d.parent.clone()).collect();
    all.into_iter()
        .filter(|d| {
            if d.startup {
                true
            } else if !d.external {
                false
            } else if d.whole {
                d.fs.is_empty() && !formatted.contains(&d.id)
            } else {
                !d.fs.is_empty() && d.name != "EFI"
            }
        })
        .collect()
}

/// A minimal reading of the JSON `lsblk -J` prints: enough to walk its
/// devices without a JSON library. Each device is an object of string,
/// number, boolean or null fields, with its partitions under `children`.
pub fn parse_lsblk(text: &str) -> Result<Vec<Drive>, String> {
    #[derive(Debug)]
    enum J {
        Null,
        Bool(bool),
        Num(f64),
        Str(String),
        List(Vec<J>),
        Map(Vec<(String, J)>),
    }
    struct P<'a>(std::iter::Peekable<std::str::Chars<'a>>);
    impl P<'_> {
        fn space(&mut self) {
            while self.0.peek().is_some_and(|c| c.is_whitespace()) {
                self.0.next();
            }
        }
        fn value(&mut self) -> Option<J> {
            self.space();
            match *self.0.peek()? {
                '{' => {
                    self.0.next();
                    let mut map = vec![];
                    loop {
                        self.space();
                        if self.0.peek() == Some(&'}') {
                            self.0.next();
                            return Some(J::Map(map));
                        }
                        let J::Str(key) = self.value()? else {
                            return None;
                        };
                        self.space();
                        (self.0.next()? == ':').then_some(())?;
                        map.push((key, self.value()?));
                        self.space();
                        if self.0.peek() == Some(&',') {
                            self.0.next();
                        }
                    }
                }
                '[' => {
                    self.0.next();
                    let mut list = vec![];
                    loop {
                        self.space();
                        if self.0.peek() == Some(&']') {
                            self.0.next();
                            return Some(J::List(list));
                        }
                        list.push(self.value()?);
                        self.space();
                        if self.0.peek() == Some(&',') {
                            self.0.next();
                        }
                    }
                }
                '"' => {
                    self.0.next();
                    let mut s = String::new();
                    loop {
                        match self.0.next()? {
                            '"' => return Some(J::Str(s)),
                            '\\' => match self.0.next()? {
                                'n' => s.push('\n'),
                                't' => s.push('\t'),
                                'u' => {
                                    let hex: String = (0..4).filter_map(|_| self.0.next()).collect();
                                    s.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?).unwrap_or('?'));
                                }
                                other => s.push(other),
                            },
                            c => s.push(c),
                        }
                    }
                }
                _ => {
                    let mut word = String::new();
                    while self.0.peek().is_some_and(|c| !",]} \n\t\r".contains(*c)) {
                        word.push(self.0.next()?);
                    }
                    match word.as_str() {
                        "null" => Some(J::Null),
                        "true" => Some(J::Bool(true)),
                        "false" => Some(J::Bool(false)),
                        n => n.parse().ok().map(J::Num),
                    }
                }
            }
        }
    }
    fn get<'a>(map: &'a [(String, J)], key: &str) -> Option<&'a J> {
        map.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
    fn string(map: &[(String, J)], key: &str) -> String {
        match get(map, key) {
            Some(J::Str(s)) => s.clone(),
            _ => String::new(),
        }
    }
    // Older versions print flags as "0" and "1".
    fn flag(map: &[(String, J)], key: &str) -> bool {
        match get(map, key) {
            Some(J::Bool(b)) => *b,
            Some(J::Str(s)) => s == "1",
            Some(J::Num(n)) => *n != 0.0,
            _ => false,
        }
    }
    fn walk(node: &[(String, J)], parent: Option<(&str, bool)>, out: &mut Vec<Drive>) {
        let id = string(node, "path");
        let external = flag(node, "rm") || flag(node, "hotplug") || parent.is_some_and(|p| p.1);
        let kids: &[J] = match get(node, "children") {
            Some(J::List(l)) => l,
            _ => &[],
        };
        let fs = string(node, "fstype");
        let mount = Some(string(node, "mountpoint")).filter(|m| !m.is_empty()).map(PathBuf::from);
        let size = match get(node, "size") {
            Some(J::Num(n)) => *n as u64,
            Some(J::Str(s)) => s.parse().unwrap_or(0),
            _ => 0,
        };
        let kind = string(node, "type");
        let startup = mount.as_deref().is_some_and(|m| m == std::path::Path::new("/") || m.starts_with("/boot"));
        // A volume with a file system, or an external drive with nothing on
        // it. Of the system's own, only the one it runs from: the small
        // ones it boots with are nobody's idea of a drive.
        let empty_drive = kind == "disk" && fs.is_empty() && kids.is_empty() && external;
        if !id.is_empty() && ((!fs.is_empty() && fs != "swap" && matches!(kind.as_str(), "part" | "disk")) || empty_drive) && (external || mount.as_deref() == Some(std::path::Path::new("/"))) {
            out.push(Drive { parent: parent.map_or(id.clone(), |p| p.0.to_owned()), name: string(node, "label"), whole: kind == "disk", id: id.clone(), size, fs, mount, external, startup });
        }
        for kid in kids {
            if let J::Map(m) = kid {
                walk(m, Some((if parent.is_some() { parent.map_or("", |p| p.0) } else { &id }, external)), out);
            }
        }
    }
    let Some(J::Map(top)) = P(text.chars().peekable()).value() else {
        return Err("Could not read the list of drives.".into());
    };
    let mut out = vec![];
    if let Some(J::List(devices)) = get(&top, "blockdevices") {
        for d in devices {
            if let J::Map(m) = d {
                walk(m, None, &mut out);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DISKUTIL: &str = "
**********

   Device Identifier:         disk0
   Device Node:               /dev/disk0
   Whole:                     Yes
   Part of Whole:             disk0
   Volume Name:               Not applicable (no file system)
   Mounted:                   Not applicable (no file system)
   Disk Size:                 500.3 GB (500277792768 Bytes) (exactly 977105064 512-Byte-Units)
   Device Location:           Internal
   Removable Media:           Fixed

**********

   Device Identifier:         disk3s1s1
   Whole:                     No
   Part of Whole:             disk3
   Volume Name:               Macintosh HD
   Mounted:                   Yes
   Mount Point:               /
   File System Personality:   APFS
   Disk Size:                 494.4 GB (494384795648 Bytes) (exactly 965595304 512-Byte-Units)
   Device Location:           Internal
   Removable Media:           Fixed

**********

   Device Identifier:         disk3s5
   Whole:                     No
   Part of Whole:             disk3
   Volume Name:               Data
   Mounted:                   Yes
   Mount Point:               /System/Volumes/Data
   File System Personality:   APFS
   Disk Size:                 494.4 GB (494384795648 Bytes) (exactly 965595304 512-Byte-Units)
   Device Location:           Internal
   Removable Media:           Fixed

**********

   Device Identifier:         disk6
   Whole:                     Yes
   Part of Whole:             disk6
   Volume Name:               Not applicable (no file system)
   Disk Size:                 64.0 GB (64023257088 Bytes) (exactly 125045424 512-Byte-Units)
   Device Location:           External
   Removable Media:           Removable

**********

   Device Identifier:         disk6s1
   Whole:                     No
   Part of Whole:             disk6
   Volume Name:               EFI
   Mounted:                   No
   File System Personality:   MS-DOS FAT32
   Disk Size:                 209.7 MB (209715200 Bytes) (exactly 409600 512-Byte-Units)
   Device Location:           External
   Removable Media:           Removable

**********

   Device Identifier:         disk6s2
   Whole:                     No
   Part of Whole:             disk6
   Volume Name:               HOLIDAY
   Mounted:                   Yes
   Mount Point:               /Volumes/HOLIDAY
   File System Personality:   ExFAT
   Disk Size:                 63.8 GB (63813287936 Bytes) (exactly 124635328 512-Byte-Units)
   Device Location:           External
   Removable Media:           Removable

**********

   Device Identifier:         disk7
   Whole:                     Yes
   Part of Whole:             disk7
   Volume Name:               Not applicable (no file system)
   Disk Size:                 1.0 TB (1000204886016 Bytes) (exactly 1953525168 512-Byte-Units)
   Device Location:           External
   Removable Media:           Fixed
";

    fn external(id: &str, name: &str) -> Drive {
        Drive { id: id.into(), parent: "disk6".into(), name: name.into(), size: 64_000_000_000, fs: "ExFAT".into(), mount: Some("/Volumes/X".into()), external: true, startup: false, whole: false }
    }

    #[test]
    fn the_drives_shown_are_the_startup_disk_and_what_is_plugged_in() {
        let drives = parse_diskutil(DISKUTIL);
        let ids: Vec<&str> = drives.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, ["disk3s1s1", "disk6s2", "disk7"], "not the bare internal disk, the data volume, the EFI partition, or a drive whose volume is shown");
        let start = &drives[0];
        assert!(start.startup && !start.external);
        assert_eq!((start.name.as_str(), start.size, start.fs.as_str()), ("Macintosh HD", 494_384_795_648, "APFS"));
        let stick = &drives[1];
        assert_eq!((stick.title(), stick.parent.as_str(), stick.mount.as_deref()), ("HOLIDAY", "disk6", Some(std::path::Path::new("/Volumes/HOLIDAY"))));
        assert!(stick.external && !stick.whole && !stick.startup);
        let blank = &drives[2];
        assert!(blank.whole && blank.fs.is_empty() && blank.mount.is_none());
        assert_eq!(blank.title(), "disk7", "with no name it goes by what the system calls it");
        assert!(parse_diskutil("").is_empty());
    }

    /// Reads this computer's own drives. It changes nothing, but what it
    /// finds depends on the computer, so it is run by hand:
    /// `cargo test -p neo-disk -- --ignored --nocapture this_computer`.
    #[test]
    #[ignore]
    fn this_computers_drives_are_listed() {
        let found = list().expect("the drives are listed");
        for d in &found {
            println!("{:<14} {:<22} {:>14} {:<16} external {:<5} startup {:<5} whole {:<5} {:?}", d.id, d.title(), d.size, d.fs, d.external, d.startup, d.whole, d.mount);
        }
        assert_eq!(found.iter().filter(|d| d.startup && d.mount.as_deref() == Some(std::path::Path::new("/"))).count(), 1, "one startup volume");
        assert!(found.iter().all(|d| plain_id(&d.id) && plain_id(&d.parent) && d.size > 0));
        assert!(found.iter().filter(|d| d.startup).all(|d| format_refusal(d).is_some()));
    }

    #[test]
    fn linux_drives_are_read_from_lsblk() {
        let text = r#"{ "blockdevices": [
          {"name":"nvme0n1","path":"/dev/nvme0n1","size":512110190592,"fstype":null,"label":null,"mountpoint":null,"rm":false,"hotplug":false,"type":"disk",
            "children":[
              {"name":"nvme0n1p1","path":"/dev/nvme0n1p1","size":536870912,"fstype":"vfat","label":null,"mountpoint":"/boot/efi","rm":false,"hotplug":false,"type":"part"},
              {"name":"nvme0n1p2","path":"/dev/nvme0n1p2","size":500000000000,"fstype":"ext4","label":"root","mountpoint":"/","rm":false,"hotplug":false,"type":"part"},
              {"name":"nvme0n1p3","path":"/dev/nvme0n1p3","size":8000000000,"fstype":"swap","label":null,"mountpoint":"[SWAP]","rm":false,"hotplug":false,"type":"part"}
            ]},
          {"name":"sdb","path":"/dev/sdb","size":64023257088,"fstype":null,"label":null,"mountpoint":null,"rm":"1","hotplug":"1","type":"disk",
            "children":[{"name":"sdb1","path":"/dev/sdb1","size":64000000000,"fstype":"exfat","label":"HOLIDAY é","mountpoint":null,"rm":"1","hotplug":"1","type":"part"}]},
          {"name":"sdc","path":"/dev/sdc","size":1000204886016,"fstype":null,"label":null,"mountpoint":null,"rm":false,"hotplug":true,"type":"disk"}
        ] }"#;
        let drives = parse_lsblk(text).unwrap();
        let ids: Vec<&str> = drives.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, ["/dev/nvme0n1p2", "/dev/sdb1", "/dev/sdc"], "the volume the system runs from, and external ones; not the boot partition, not swap, not a drive whose partition is shown");
        assert!(drives[0].startup && !drives[0].external);
        let stick = &drives[1];
        assert_eq!((stick.name.as_str(), stick.parent.as_str(), stick.fs.as_str(), stick.size), ("HOLIDAY é", "/dev/sdb", "exfat", 64_000_000_000));
        assert!(stick.external && stick.mount.is_none() && !stick.whole, "flags written as \"1\" count too");
        assert!(drives[2].whole && drives[2].external && drives[2].fs.is_empty());
        assert!(parse_lsblk("not json").is_err());
        assert!(parse_lsblk("{}").unwrap().is_empty());
    }

    #[test]
    fn only_external_drives_may_be_formatted() {
        let stick = external(if cfg!(target_os = "macos") { "disk6s2" } else { "/dev/sdb1" }, "HOLIDAY");
        assert_eq!(format_refusal(&stick), None);
        let format = Action::Format { fs: Fs::available().first().copied().unwrap_or(Fs::ExFat), name: "New".into() };
        if !cfg!(windows) {
            assert!(commands(&stick, &format).is_ok());
        }
        // The startup disk, however it is described, and anything internal.
        for (startup, external) in [(true, false), (true, true), (false, false)] {
            let d = Drive { startup, external, ..stick.clone() };
            assert!(format_refusal(&d).is_some());
            let refused = commands(&d, &format);
            assert!(refused.is_err(), "startup {startup}, external {external}: {refused:?}");
        }
    }

    #[test]
    fn identifiers_that_are_not_the_systems_own_are_never_run() {
        let odd = if cfg!(target_os = "macos") { vec!["", "disk", "disk6; rm -rf /", "/dev/disk6", "disks", "../disk6", "disk6 s2"] } else { vec!["", "sdb1", "/dev/", "/dev/sdb1; reboot", "/dev/../etc/passwd", "/dev/sd b"] };
        for id in odd {
            let d = Drive { id: id.into(), ..external("x", "X") };
            assert!(commands(&d, &Action::Unmount).is_err(), "{id:?}");
            assert!(commands(&d, &Action::Format { fs: Fs::ExFat, name: "New".into() }).is_err(), "{id:?}");
        }
        // Ejecting uses the whole drive's identifier, which is checked too.
        let d = Drive { parent: "nonsense; true".into(), ..external(if cfg!(target_os = "macos") { "disk6s2" } else { "/dev/sdb1" }, "X") };
        assert!(commands(&d, &Action::Eject).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_commands_are_diskutils() {
        let stick = external("disk6s2", "HOLIDAY");
        let words = |action| commands(&stick, &action).unwrap();
        assert_eq!(words(Action::Mount), [["diskutil", "mount", "disk6s2"]]);
        assert_eq!(words(Action::Unmount), [["diskutil", "unmount", "disk6s2"]]);
        assert_eq!(words(Action::Eject), [["diskutil", "eject", "disk6"]], "the whole drive, not one volume of it");
        assert_eq!(words(Action::Format { fs: Fs::ExFat, name: " Trip 2026 ".into() }), [["diskutil", "eraseVolume", "ExFAT", "Trip 2026", "disk6s2"]]);
        assert_eq!(words(Action::Format { fs: Fs::Fat32, name: "camera".into() }), [["diskutil", "eraseVolume", "MS-DOS FAT32", "CAMERA", "disk6s2"]]);
        assert_eq!(words(Action::Format { fs: Fs::MacJournaled, name: "Old Mac".into() }), [["diskutil", "eraseVolume", "Journaled HFS+", "Old Mac", "disk6s2"]]);
        // A drive with nothing on it gets a layout as well as a file system.
        let blank = Drive { id: "disk7".into(), parent: "disk7".into(), whole: true, fs: String::new(), mount: None, ..stick.clone() };
        assert_eq!(commands(&blank, &Action::Format { fs: Fs::Apfs, name: "Backup".into() }).unwrap(), [["diskutil", "eraseDisk", "APFS", "Backup", "disk7"]]);
        assert!(commands(&stick, &Action::Format { fs: Fs::Ext4, name: "x".into() }).is_err(), "not something macOS can make");
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn the_commands_are_udisks_and_mkfs() {
        let stick = Drive { id: "/dev/sdb1".into(), parent: "/dev/sdb".into(), ..external("x", "HOLIDAY") };
        let words = |action| commands(&stick, &action).unwrap();
        assert_eq!(words(Action::Mount), [["udisksctl", "mount", "-b", "/dev/sdb1"]]);
        assert_eq!(words(Action::Eject), [vec!["udisksctl", "unmount", "-b", "/dev/sdb1"], vec!["udisksctl", "power-off", "-b", "/dev/sdb"]]);
        // Mounted, so it is unmounted first; then the administrator is asked.
        assert_eq!(words(Action::Format { fs: Fs::Fat32, name: "camera".into() }), [vec!["udisksctl", "unmount", "-b", "/dev/sdb1"], vec!["pkexec", "mkfs.vfat", "-F", "32", "-n", "CAMERA", "/dev/sdb1"]]);
        let loose = Drive { mount: None, ..stick.clone() };
        assert_eq!(commands(&loose, &Action::Format { fs: Fs::Ext4, name: "data".into() }).unwrap(), [["pkexec", "mkfs.ext4", "-F", "-L", "data", "/dev/sdb1"]]);
    }

    #[test]
    fn names_are_checked_against_what_each_file_system_allows() {
        assert_eq!(Fs::ExFat.label("  Trip  "), Ok("Trip".into()));
        assert_eq!(Fs::Fat32.label("camera 1"), Ok("CAMERA 1".into()), "FAT32 keeps capitals only");
        assert!(Fs::Fat32.label("twelve chars").is_err(), "eleven at most");
        assert!(Fs::Fat32.label("café").is_err());
        assert!(Fs::ExFat.label("sixteen letters!").is_err());
        assert_eq!(Fs::Apfs.label("A long name is fine here"), Ok("A long name is fine here".into()));
        for bad in ["", "   ", "a/b", "a:b", "tab\there", "what?"] {
            assert!(Fs::ExFat.label(bad).is_err(), "{bad:?}");
        }
        assert!(Fs::available().iter().all(|f| !f.name().is_empty() && !f.note().is_empty()));
    }

    #[test]
    fn a_failing_command_reports_what_it_said_and_stops() {
        if cfg!(windows) {
            return;
        }
        let words = |w: &[&str]| w.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(run(&[words(&["sh", "-c", "echo first"]), words(&["sh", "-c", "echo done"])]), Ok("done".into()));
        let failed = run(&[words(&["sh", "-c", "echo no such drive >&2; exit 1"]), words(&["sh", "-c", "echo must not run; touch /nonexistent/x"])]);
        assert_eq!(failed, Err("no such drive".into()));
        assert!(run(&[words(&["neo-disk-no-such-program"])]).unwrap_err().starts_with("Could not run"));
        assert_eq!(run(&[]), Ok(String::new()));
    }
}
