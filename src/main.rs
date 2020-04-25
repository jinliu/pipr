#[macro_use]
extern crate maplit;
extern crate getopts;
use atty::Stream;
use getopts::Options;
use itertools::Itertools;
use std::env;
use std::fs::File;
use std::io;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use tui::{backend::CrosstermBackend, Terminal};

use crossterm::{
    event::{self, Event as CEvent},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};

pub mod app;
pub mod command_evaluation;
pub mod command_template;
pub mod commandlist;
pub mod lineeditor;
pub mod pipr_config;
pub mod snippets;
pub mod ui;
pub mod util;

pub use app::app::*;
pub use command_evaluation::*;
pub use commandlist::CommandList;
use io::Read;
pub use lineeditor as le;
pub use pipr_config::*;

pub struct CliArgs {
    default_content: Option<String>,
    output_file: Option<String>,
    input_file: Option<String>,
    unsafe_mode: bool,
    raw_mode: bool,
}

#[tokio::main]
async fn main() -> Result<(), failure::Error> {
    let args = handle_cli_arguments();

    pub const CONFIG_DIR_RELATIVE_TO_HOME: &'static str = ".config/pipr/";
    let config_path = Path::new(&env::var("HOME").unwrap()).join(CONFIG_DIR_RELATIVE_TO_HOME);

    let config = PiprConfig::load_from_file(&config_path.join("pipr.toml"));

    let execution_mode = if args.unsafe_mode {
        ExecutionMode::UNSAFE
    } else {
        ExecutionMode::ISOLATED
    };

    let bubblewrap_available = which::which("bwrap").is_ok();

    if !bubblewrap_available && execution_mode != ExecutionMode::UNSAFE {
        println!("bubblewrap installation not found. Please make sure you have `bwrap` on your path, or supply --no-isolation to disable safe-mode");
        std::process::exit(1);
    }

    let execution_handler = CommandExecutionHandler::start(execution_mode, config.eval_environment.clone());

    let bookmarks = CommandList::load_from_file(config_path.join("bookmarks"), None);
    let history = CommandList::load_from_file(config_path.join("history"), Some(config.history_size));

    // create app and set default

    let mut app = App::new(execution_handler, args.raw_mode, config.clone(), bookmarks, history);

    if let Some(default_value) = args.default_content {
        app.input_state.set_content(default_value.lines().map_into().collect());
    }
    if let Some(input_file) = args.input_file {
        let mut buffer = String::new();
        File::open(input_file)?.read_to_string(&mut buffer)?;
        app.input_state.set_content(buffer.lines().map_into().collect());
    }

    // render on stdout if output is not piped into something. if it is, use stderr.
    if atty::is(Stream::Stdout) {
        run_app(&mut app, io::stdout()).await?;
    } else {
        run_app(&mut app, io::stderr()).await?;
    }

    after_finish(&app, args.output_file)?;

    Ok(())
}

/// parses the arguments, handles printing help and config-reference if requested
/// and otherwise returns an CliArgs instance.
fn handle_cli_arguments() -> CliArgs {
    let cli_args: Vec<String> = env::args().collect();
    let program = cli_args[0].clone();

    // arguments
    let mut opts = Options::new();
    opts.optopt("d", "default", "text inserted into the textfield on startup", "TEXT");
    opts.optopt("o", "out-file", "write final command to file", "FILE");
    opts.optopt("", "in-file", "read initial command from file", "FILE");
    opts.optflag("", "config-reference", "print out the default configuration file");
    opts.optflag("r", "raw-mode", "keep linebreaks in finished command when closing");
    opts.optflag(
        "",
        "no-isolation",
        "disable isolation. This will run the commands directly on your system, without protection. Take care.",
    );
    opts.optflag("h", "help", "print this help menu");

    let matches = opts.parse(&cli_args[1..]).unwrap();

    if matches.opt_present("help") {
        let brief = format!("Usage: {} [options]", program);
        print!("{}", opts.usage(&brief));
        std::process::exit(0);
    } else if matches.opt_present("config-reference") {
        println!("{}", pipr_config::DEFAULT_CONFIG);
        std::process::exit(0);
    }

    CliArgs {
        default_content: matches.opt_str("default"),
        output_file: matches.opt_str("out-file"),
        input_file: matches.opt_str("in-file"),
        unsafe_mode: matches.opt_present("no-isolation"),
        raw_mode: matches.opt_present("raw-mode"),
    }
}

/// executed after the program has been closed.
/// optionally given out_file, a path to a file that the
/// final command will be written to (mostly for scripting stuff)
fn after_finish(app: &App, out_file: Option<String>) -> Result<(), failure::Error> {
    let finished_command = if app.raw_mode {
        app.input_state.content_lines().join("\n")
    } else {
        app.input_state.content_str()
    };

    if let Some(finish_hook) = &app.config.finish_hook {
        let mut finish_hook = finish_hook.split(" ");
        if let Some(cmd) = finish_hook.next() {
            let mut child = Command::new(cmd).args(finish_hook).stdin(Stdio::piped()).spawn()?;
            let stdin = child.stdin.as_mut().unwrap();
            stdin.write_all(&finished_command.as_bytes())?;
            child.wait()?;
        }
    }

    println!("{}", finished_command);
    if let Some(out_file) = out_file {
        File::create(out_file)?.write_all(finished_command.as_bytes())?;
    }
    Ok(())
}

async fn run_app<W: Write>(mut app: &mut App, mut output_stream: W) -> Result<(), failure::Error> {
    execute!(output_stream, EnterAlternateScreen)?;
    enable_raw_mode()?;
    let backend = CrosstermBackend::new(output_stream);
    let mut terminal = Terminal::new(backend)?;

    std::panic::set_hook(Box::new(|data| {
        disable_raw_mode().unwrap();
        execute!(io::stdout(), LeaveAlternateScreen).unwrap();
        execute!(io::stderr(), LeaveAlternateScreen).unwrap();
        eprintln!("{}", data);
        std::process::exit(1);
    }));

    while !app.should_quit {
        ui::draw_app(&mut terminal, &mut app).await?;

        loop {
            if let Some(cmd_output) = app.execution_handler.poll_output() {
                app.on_cmd_output(cmd_output);
                break;
            }

            if let Ok(true) = event::poll(std::time::Duration::from_millis(100)) {
                match event::read()? {
                    CEvent::Resize(_, _) => break,
                    CEvent::Key(evt) => {
                        app.on_tui_event(evt.code, evt.modifiers).await;
                        break;
                    }
                    _ => {}
                }
            }
        }
    }
    app.execution_handler.stop().await;
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    std::io::Write::flush(&mut terminal.backend_mut())?;

    Ok(())
}
