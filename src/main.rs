use clap::Parser;
use eyre::{Context, ContextCompat, bail};
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::process::{Command, Stdio};
use tokio::task::JoinSet;

#[derive(Debug, Clone)]
struct Derivation {
    outputs: Vec<String>,
    inputs: Vec<String>,
}

#[derive(Debug)]
enum PathResult {
    Valid,
    Failed,
    Missing,
    Error,
}

#[derive(Debug)]
enum PathStatus {
    Valid,
    Failed,
    Missing,
    JoinError(tokio::task::JoinError),
    ReqwestError(reqwest::Error),
    HttpError(reqwest::StatusCode),
}

impl PathStatus {
    fn is_conclusive(&self) -> bool {
        matches!(self, PathStatus::Valid | PathStatus::Failed)
    }

    fn to_result(&self) -> PathResult {
        match self {
            PathStatus::Valid => PathResult::Valid,
            PathStatus::Failed => PathResult::Failed,
            PathStatus::Missing => PathResult::Missing,
            PathStatus::JoinError(_) => PathResult::Error,
            PathStatus::ReqwestError(_) => PathResult::Error,
            PathStatus::HttpError(_) => PathResult::Error,
        }
    }
}

impl From<tokio::task::JoinError> for PathStatus {
    fn from(error: tokio::task::JoinError) -> Self {
        PathStatus::JoinError(error)
    }
}

impl From<reqwest::Error> for PathStatus {
    fn from(error: reqwest::Error) -> Self {
        PathStatus::ReqwestError(error)
    }
}

#[derive(Debug)]
struct PathInfo {
    path: String,
    status: PathStatus,
}

#[derive(clap::Parser)]
#[command(about = "Does awesome things", long_about = None)]
struct CommandArgs {
    /// Arguments to `nix derivation show`
    #[arg(required = true)]
    args: Vec<OsString>,

    /// URL to binary cache
    #[arg(long, default_value = "https://cache.nixos.org")]
    url: reqwest::Url,

    /// Print more debugging output
    #[arg(short, long)]
    verbose: bool,
}

fn make_narinfo_url(base: &reqwest::Url, out_path: &str) -> reqwest::Url {
    let hash_part = if let Some((hash_part, _)) = out_path.split_once('-') {
        hash_part
    } else {
        out_path
    };

    base.join(&format!("{hash_part}.narinfo")).unwrap()
}

fn make_log_url(base: &reqwest::Url, drv_path: &str) -> reqwest::Url {
    base.join("log/").unwrap().join(drv_path).unwrap()
}

async fn query_derivation(
    client: &'static reqwest::Client,
    narinfo_url: reqwest::Url,
    log_url: reqwest::Url,
) -> Result<PathStatus, PathStatus> {
    let narinfo_task = tokio::spawn(client.head(narinfo_url).send());
    let status = narinfo_task.await??.status();
    let res = match status.as_u16() {
        404 => {
            let log_task = tokio::spawn(client.head(log_url).send());
            let status = log_task.await??.status();
            match status.as_u16() {
                200 => PathStatus::Failed,
                404 => PathStatus::Missing,
                _ => PathStatus::HttpError(status),
            }
        }
        200 => PathStatus::Valid,
        _ => PathStatus::HttpError(status),
    };
    Ok(res)
}

#[tokio::main(flavor = "local")]
async fn main() -> eyre::Result<()> {
    let user_agent = [
        env!("CARGO_PKG_NAME"),
        "/",
        env!("CARGO_PKG_VERSION"),
        option_env!("NIX_LOG_CHECK_VERSION_SUFFIX").unwrap_or(""),
    ]
    .join("");

    eprintln!("[INFO] {user_agent}");

    let command_args: CommandArgs = CommandArgs::parse();

    let output = Command::new("nix")
        .args([
            "--extra-experimental-features",
            "nix-command",
            "derivation",
            "show",
            "--recursive",
        ])
        .args(command_args.args)
        .stdin(Stdio::inherit())
        .stderr(Stdio::inherit())
        .output()?;
    if !output.status.success() {
        bail!("Nix command failed with status {}", output.status);
    }

    let output = output.stdout;

    let data: Value = serde_json::from_slice(&output).wrap_err("Parsing Nix output JSON")?;

    check_data_version(&data)?;

    let parsed = parse_data(&data).context("Failed to parse input data")?;

    for (path, drv) in &parsed {
        if !drv.inputs.iter().all(|p| parsed.contains_key(p)) {
            bail!(format!(
                "Nix output is incomplete: missing input for {path}"
            ));
        }
    }

    let used_as_input: HashSet<&String> = parsed.values().flat_map(|drv| &drv.inputs).collect();
    let roots: Vec<&String> = parsed
        .keys()
        .filter(|&path| !used_as_input.contains(path))
        .collect();

    let client = reqwest::Client::builder().user_agent(user_agent).build()?;
    let client: &reqwest::Client = Box::leak(Box::new(client));

    let mut seen: HashSet<String> = HashSet::new();

    let mut spawn_task_for = |path: &str, js: &mut JoinSet<PathInfo>| {
        if seen.contains(path) {
            return;
        }
        seen.insert(path.to_owned());

        let out_path = parsed.get(path).unwrap().outputs[0].to_owned();
        let path = path.to_owned();

        let narinfo_url = make_narinfo_url(&command_args.url, &out_path);
        let log_url = make_log_url(&command_args.url, &path);

        if command_args.verbose {
            eprintln!("[DEBUG] Querying {log_url} and {narinfo_url}");
        }

        js.spawn(async move {
            let status = query_derivation(client, narinfo_url, log_url)
                .await
                .unwrap_or_else(|error| error);
            PathInfo {
                path: path.to_owned(),
                status,
            }
        });
    };

    eprintln!(
        "[INFO] Checking {} root derivation(s), total closure size {}",
        roots.len(),
        parsed.len()
    );

    let js = &mut JoinSet::new();

    for drv in roots {
        spawn_task_for(drv, js);
    }

    let mut results: HashMap<String, PathResult> = HashMap::new();

    while let Some(res) = js.join_next().await {
        let PathInfo { path, status } = res?;
        let drv: &Derivation = parsed.get(&path).unwrap();

        results.insert(path.clone(), status.to_result());

        let conclusive = status.is_conclusive();

        match status {
            PathStatus::Failed => eprintln!("[INFO] Possibly failing: {path}"),
            PathStatus::JoinError(e) => Err(e).context(format!("Error checking {path}"))?,
            PathStatus::ReqwestError(e) => Err(e).context(format!("Error checking {path}"))?,
            PathStatus::HttpError(sc) if sc.is_client_error() => {
                bail!("{sc} checking {path}\nPlease check binary cache URL or try again later.")
            }
            PathStatus::HttpError(sc) => eprintln!("[ERROR] HTTP error {sc}: {path}"),
            PathStatus::Valid | PathStatus::Missing => {}
        }

        if !conclusive {
            for input in &drv.inputs {
                spawn_task_for(input, js);
            }
        }
    }

    let num_missing = results
        .iter()
        .filter(|(_, v)| !matches!(v, PathResult::Valid))
        .count();

    let failed: Vec<String> = results
        .into_iter()
        .filter_map(|(k, v)| matches!(v, PathResult::Failed).then_some(k))
        .collect();

    let num_failed = failed.len();

    eprintln!("[INFO] {num_missing} path(s) not in binary cache");

    if num_failed > 0 {
        eprintln!("[INFO] {num_failed} path(s) possibly failing");
    } else {
        eprintln!("[INFO] No possibly failing path found");
    }

    println!("{}", serde_json::to_string_pretty(&failed)?);

    Ok(())
}

fn check_data_version(data: &Value) -> eyre::Result<()> {
    let Some(obj) = data.as_object() else {
        bail!("Invalid data in Nix output");
    };
    let Some(version) = obj.get("version").and_then(|v| v.as_number()) else {
        bail!("Invalid version in Nix output (Nix version too old)");
    };
    let version = version.as_i64().context("Invaild version in Nix output")?;

    if version < 4 {
        bail!("Invalid version {version} in Nix output (Nix version too old)");
    }

    if version > 4 {
        bail!(
            "Invalid version {version} in Nix output (Nix version too new)\nPlease report this as a bug."
        );
    }

    Ok(())
}

fn parse_derivation(data: &Value) -> eyre::Result<Derivation> {
    let data = data.as_object().context("Derivation is not an object")?;
    let inputs: Map<String, Value> = serde_json::from_value(
        data.get("inputs")
            .context("No inputs key")?
            .as_object()
            .context("Value of inputs is not an object")?
            .get("drvs")
            .context("No drvs key")?
            .clone(),
    )
    .wrap_err("Parsing derivation inputs")?;
    let inputs: Vec<String> = inputs.keys().cloned().collect();
    let outputs_map: Map<String, Value> =
        serde_json::from_value(data.get("outputs").context("No outputs key")?.clone())
            .ok()
            .wrap_err("Parsing derivation outputs")?;
    let outputs: Option<Vec<String>> = outputs_map
        .values()
        .map(|v| -> Option<String> { Some(v.as_object()?.get("path")?.as_str()?.to_owned()) })
        .collect();
    let outputs = outputs
        .or_else(|| {
            let path = data.get("env")?.as_object()?.get("out")?.as_str()?;
            let path = if let Some((_, base)) = path.rsplit_once('/') {
                base
            } else {
                path
            };

            Some(vec![path.to_owned()])
        })
        .wrap_err("Parsing derivation outputs")?;
    if outputs.is_empty() {
        bail!("Derivation has no outputs");
    }
    Ok(Derivation { outputs, inputs })
}

fn parse_data(data: &Value) -> eyre::Result<HashMap<String, Derivation>> {
    let data = data.as_object().context("JSON is not an object")?;
    let derivations = data
        .get("derivations")
        .context("No derivations key")?
        .as_object()
        .context("Value of derivations is not an object")?;
    derivations
        .iter()
        .map(|(name, value)| {
            Ok((
                name.clone(),
                parse_derivation(value).context(format!("Parsing data for derivation {name}"))?,
            ))
        })
        .collect()
}
