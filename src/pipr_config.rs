use std::env;
use std::fs::{DirBuilder, File};
use std::io::prelude::*;
use std::path::{Path, PathBuf};

const DEFAULT_CONFIG: &str = "
#  ____  _
# |  _ \\(_)_ __  _ __      .__________.
# | |_) | | '_ \\| '__|     |__________|
# |  __/| | |_) | |          |_    -|
# |_|   |_| .__/|_|     xX  xXx___x_|
#         |_|
#
# A commandline utility by 
# Leon Kowarschick

# finish_hook: Executed once you close pipr, getting the command you constructed piped into stdin.
# finish_hook = \"xclip -selection clipboard -in\"

# Show the help-sidebar by default
show_help = true

# directories mounted into the isolated environment.
# Syntax: '<on_host>:<in_isolated>'
isolation_mounts_readonly = ['/lib:/lib', '/usr:/usr', '/lib64:/lib64', '/bin:/bin', '/etc:/etc']
";
const CONFIG_PATH_RELATIVE_TO_HOME: &'static str = ".config/pipr/pipr.toml";

#[derive(Debug, Clone)]
pub struct PiprConfig {
    pub finish_hook: Option<String>,
    pub show_help: bool,
    pub isolation_mounts_readonly: Vec<(String, String)>,
}

impl PiprConfig {
    pub fn load_from_file() -> PiprConfig {
        let home_path = env::var("HOME").unwrap();
        let config_path = Path::new(&home_path).join(CONFIG_PATH_RELATIVE_TO_HOME);
        DirBuilder::new()
            .recursive(true)
            .create(&config_path.parent().unwrap())
            .unwrap();
        if !config_path.exists() {
            create_default_file(&config_path);
        }

        let mut settings = config::Config::default();
        let config_file = config::File::new(config_path.to_str().unwrap(), config::FileFormat::Toml);
        settings.merge(config_file).unwrap();
        PiprConfig {
            finish_hook: settings.get::<String>("finish_hook").ok(),
            show_help: settings.get::<bool>("show_help").ok().unwrap_or(true),
            isolation_mounts_readonly: parse_isolation_mounts(
                &settings.get::<Vec<String>>("isolation_mounts_readonly").unwrap_or(vec![
                    "/lib:/lib".into(),
                    "/usr:/usr".into(),
                    "/lib64:/lib64".into(),
                    "/bin:/bin".into(),
                    "/etc:/etc".into(),
                ]),
            ),
        }
    }
}

fn parse_isolation_mounts(entries: &Vec<String>) -> Vec<(String, String)> {
    let parse_error_msg = "Invalid format in mount configuration. Format: '<on-host>:<in-isolated>'";
    entries
        .iter()
        .map(|entry| entry.split(':').collect::<Vec<&str>>())
        .map(|vec| {
            (
                vec.get(0).expect(parse_error_msg).to_string(),
                vec.get(1).expect(parse_error_msg).to_string(),
            )
        })
        .collect::<Vec<(String, String)>>()
}

fn create_default_file(path: &PathBuf) {
    let mut file = File::create(path).unwrap();
    file.write_all(DEFAULT_CONFIG.as_bytes()).unwrap();
}