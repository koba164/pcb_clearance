use crate::parser::metadata;
use anyhow::{Context, Result, bail, ensure};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    io::Read,
    ops::Deref,
    path::{Path, PathBuf},
    sync::LazyLock,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Skipped {
    pub path: String,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum Source {
    File,
    Zip {
        archive: PathBuf,
        index: usize,
        name: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct InputFile {
    path: PathBuf,
    source: Source,
}
impl Deref for InputFile {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.path
    }
}
impl InputFile {
    pub(crate) fn identity(&self) -> String {
        match &self.source {
            Source::File => format!("file:{}", self.display()),
            Source::Zip {
                archive,
                index,
                name,
            } => format!("zip:{}!/{index}/{name}", archive.display()),
        }
    }
    pub fn display_name(&self) -> String {
        match &self.source {
            Source::File => self
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            Source::Zip { archive, name, .. } => format!(
                "{}!/{name}",
                archive.file_name().unwrap_or_default().to_string_lossy()
            ),
        }
    }
    pub fn read_bytes(&self) -> Result<Vec<u8>> {
        match &self.source {
            Source::File => Ok(fs::read(&self.path)
                .with_context(|| format!("入力を読めません: {}", self.display()))?),
            Source::Zip {
                archive,
                index,
                name,
            } => {
                let mut zip = zip::ZipArchive::new(fs::File::open(archive)?)
                    .with_context(|| format!("ZIP を開けません: {}", archive.display()))?;
                let mut member = zip
                    .by_index(*index)
                    .with_context(|| format!("ZIP 内のファイルを読めません: {}", self.display()))?;
                ensure!(
                    member.name() == name,
                    "選択後に ZIP の内容が変わりました。追加し直してください: {}",
                    archive.display()
                );
                let mut bytes = vec![];
                member
                    .read_to_end(&mut bytes)
                    .with_context(|| format!("ZIP 内のファイルを読めません: {}", self.display()))?;
                Ok(bytes)
            }
        }
    }
    pub fn size(&self) -> u64 {
        match &self.source {
            Source::File => fs::metadata(&self.path).map(|v| v.len()).unwrap_or(0),
            Source::Zip { archive, index, .. } => fs::File::open(archive)
                .ok()
                .and_then(|f| zip::ZipArchive::new(f).ok())
                .and_then(|mut zip| zip.by_index(*index).ok().map(|v| v.size()))
                .unwrap_or(0),
        }
    }
}
static COPPER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\.(gtl|gbl|g\d+|gp\d+)$|(?:^|[._-])(?:f|b|in\d+)[._-]cu(?:[._-]|$)").unwrap()
});
static NON_COPPER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\.(gts|gbs|gto|gbo|gtp|gbp|gd\d+|gg\d+|gm\d+|drl|txt|zip|gko)$").unwrap()
});
pub fn classify(path: &Path, text: &str) -> (Option<bool>, String) {
    if let Some(function) = metadata(text).get("FileFunction") {
        return (
            Some(function.split(',').next() == Some("Copper")),
            format!("X2 FileFunction={function}"),
        );
    }
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    if COPPER.is_match(&name) {
        (Some(true), "銅箔のファイル名".into())
    } else if NON_COPPER.is_match(&name) {
        (
            Some(false),
            "レジスト・シルク・ペースト・ドリル・機械層など".into(),
        )
    } else {
        (
            None,
            "銅箔層を自動判別できない名前（必要ならファイルを直接指定）".into(),
        )
    }
}
fn select(
    file: InputFile,
    bytes: &[u8],
    explicit: bool,
    strict_fs: bool,
    selected: &mut BTreeSet<InputFile>,
    skipped: &mut Vec<Skipped>,
) -> Result<()> {
    let text = String::from_utf8_lossy(bytes);
    let (copper, reason) = classify(&file, &text);
    if copper == Some(true) {
        selected.insert(file);
        return Ok(());
    }
    if !text.contains("%FS") {
        if strict_fs {
            bail!("RS-274X の FS 指定がない: {}", file.display());
        }
        skipped.push(Skipped {
            path: file.display().to_string(),
            reason: "RS-274X ではない".into(),
        });
    } else if copper.is_none() && explicit {
        selected.insert(file);
    } else {
        skipped.push(Skipped {
            path: file.display().to_string(),
            reason,
        });
    }
    Ok(())
}
fn add_zip(
    path: &Path,
    selected: &mut BTreeSet<InputFile>,
    skipped: &mut Vec<Skipped>,
) -> Result<()> {
    let archive = absolute(path)?;
    let mut zip = zip::ZipArchive::new(fs::File::open(&archive)?)
        .with_context(|| format!("ZIP を開けません: {}", archive.display()))?;
    let mut names = BTreeSet::new();
    for index in 0..zip.len() {
        let mut member = zip
            .by_index(index)
            .with_context(|| format!("ZIP 内の項目を読めません: {}", archive.display()))?;
        let enclosed = member.enclosed_name().with_context(|| {
            format!(
                "ZIP 内のパスが不正です: {}!/{}",
                archive.display(),
                member.name()
            )
        })?;
        if member.is_dir() {
            continue;
        }
        let name = member.name().to_string();
        ensure!(
            names.insert(enclosed.clone()),
            "ZIP 内のパスが重複しています: {}!/{name}",
            archive.display()
        );
        let path = PathBuf::from(format!(
            "{}!/{}",
            archive.display(),
            enclosed.to_string_lossy()
        ));
        let file = InputFile {
            path,
            source: Source::Zip {
                archive: archive.clone(),
                index,
                name,
            },
        };
        if file
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
        {
            skipped.push(Skipped {
                path: file.display().to_string(),
                reason: "ZIP 内の ZIP は検索対象外です".into(),
            });
            continue;
        }
        // 展開ファイルを作らず、ZIP 内のガーバーをそのまま読み込む。
        let mut bytes = vec![];
        member
            .read_to_end(&mut bytes)
            .with_context(|| format!("ZIP 内のファイルを読めません: {}", file.display()))?;
        select(file, &bytes, false, false, selected, skipped)?;
    }
    Ok(())
}
fn visit(
    path: &Path,
    explicit: bool,
    strict_fs: bool,
    visited: &mut BTreeSet<PathBuf>,
    selected: &mut BTreeSet<InputFile>,
    skipped: &mut Vec<Skipped>,
) -> Result<()> {
    let absolute = absolute(path)?;
    if path.is_dir() {
        // 同じ実フォルダーを一度だけ調べ、リンクの循環と重複選択を避ける。
        if !visited.insert(absolute) {
            return Ok(());
        }
        let mut children = fs::read_dir(path)?
            .map(|e| e.map(|e| e.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        children.sort();
        for child in children {
            visit(&child, false, false, visited, selected, skipped)?;
        }
    } else if path.is_file() {
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
        {
            if visited.insert(absolute) {
                add_zip(path, selected, skipped)?;
            }
        } else {
            let bytes = fs::read(path)?;
            select(
                InputFile {
                    path: absolute,
                    source: Source::File,
                },
                &bytes,
                explicit,
                strict_fs,
                selected,
                skipped,
            )?;
        }
    }
    Ok(())
}
pub fn discover(inputs: &[PathBuf]) -> Result<(Vec<InputFile>, Vec<Skipped>)> {
    let mut selected = BTreeSet::new();
    let mut skipped = vec![];
    let mut visited = BTreeSet::new();
    for input in inputs {
        let hits = if input.exists() {
            vec![input.clone()]
        } else {
            let mut hits = glob::glob(input.to_str().context("ワイルドカードのパスが不正")?)?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            hits.sort();
            hits
        };
        ensure!(!hits.is_empty(), "入力が見つからない: {}", input.display());
        for hit in &hits {
            visit(
                hit,
                true,
                hits.len() == 1,
                &mut visited,
                &mut selected,
                &mut skipped,
            )?;
        }
    }
    ensure!(
        !selected.is_empty(),
        "銅箔ガーバーが見つからない。名前の不明な層はファイルを直接指定してください"
    );
    Ok((selected.into_iter().collect(), skipped))
}
pub fn absolute(path: &Path) -> Result<PathBuf> {
    let p = fs::canonicalize(path)?;
    #[cfg(windows)]
    {
        let s = p.to_string_lossy();
        if let Some(v) = s.strip_prefix(r"\\?\UNC\") {
            return Ok(PathBuf::from(format!(r"\\{v}")));
        }
        if let Some(v) = s.strip_prefix(r"\\?\") {
            return Ok(PathBuf::from(v));
        }
    }
    Ok(p)
}
