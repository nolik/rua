#[global_allocator]
static GLOBAL: std::alloc::System = std::alloc::System;

mod action_jailbuild;
mod action_search;
mod aur_download;
mod cli_args;
mod pacman;
mod print_format;
mod print_package_info;
mod print_package_table;
mod rua_dirs;
mod srcinfo_to_pkgbuild;
mod tar_check;
mod terminal_util;
mod wrapped;

use std::fs::{File, OpenOptions, Permissions};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::exit;
use std::process::Command;
use std::{env, fs};

use crate::cli_args::CLIColorType;
use crate::print_package_info::info;
use chrono::Utc;
use cli_args::{Action, CliArgs};
use directories::ProjectDirs;
use env_logger::Env;
use fs2::FileExt;
use log::debug;
use structopt::StructOpt;

fn default_env(key: &str, value: &str) {
	if env::var_os(key).is_none() {
		env::set_var(key, value);
	}
}

fn overwrite_file(path: &Path, content: &[u8]) {
	let mut file = OpenOptions::new()
		.create(true)
		.write(true)
		.truncate(true)
		.open(path)
		.unwrap_or_else(|err| panic!("Failed to overwrite (initialize) file {:?}, {}", path, err));
	file.write_all(content).unwrap_or_else(|e| {
		panic!(
			"Failed to write to file {:?} during initialization, {}",
			path, e
		)
	});
}

fn ensure_script(path: &Path, content: &[u8]) {
	if !path.exists() {
		let mut file = OpenOptions::new()
			.create(true)
			.write(true)
			.open(path)
			.unwrap_or_else(|e| panic!("Failed to overwrite (initialize) file {:?}, {}", path, e));
		file.write_all(content).unwrap_or_else(|e| {
			panic!(
				"Failed to write to file {:?} during initialization, {}",
				path, e
			)
		});
		fs::set_permissions(path, Permissions::from_mode(0o755))
			.unwrap_or_else(|e| panic!("Failed to set permissions for {:?}, {}", path, e));
	}
}

fn overwrite_script(path: &Path, content: &[u8]) {
	overwrite_file(path, content);
	fs::set_permissions(path, Permissions::from_mode(0o755))
		.unwrap_or_else(|e| panic!("Failed to set permissions for {:?}, {}", path, e));
}

fn main() {
	default_env("RUST_BACKTRACE", "1"); // if it wasn't set to "0" explicitly, set it to 1.
	env_logger::Builder::from_env(Env::default().filter_or("LOG_LEVEL", "info"))
		.format(|buf, record| {
			writeln!(
				buf,
				"{} [{}] - {}",
				Utc::now().format("%Y-%m-%d %H:%M:%S"),
				record.level(),
				record.args()
			)
		})
		.init();
	debug!(
		"{} version {}",
		env!("CARGO_PKG_NAME"),
		env!("CARGO_PKG_VERSION")
	);
	let config: CliArgs = CliArgs::from_args();
	match config.action {
		Action::Install { .. } | Action::Jailbuild { .. } => {
			if users::get_current_uid() == 0 {
				eprintln!("RUA should not be run as root.");
				eprintln!("Also, makepkg will not allow you building from root anyway.");
				exit(1)
			}
			if !Command::new("bwrap")
				.args(&["--ro-bind", "/", "/", "true"])
				.status()
				.expect("bwrap binary not found. RUA uses bubblewrap for security isolation.")
				.success()
			{
				eprintln!("Failed to run bwrap.");
				eprintln!("A possible cause for this is if RUA itself is run in jail (docker, bwrap, firejail,..).");
				eprintln!("If so, see https://github.com/vn971/rua/issues/8");
				exit(4)
			}
		}
		_ => {}
	}
	match config.color {
		// see "colored" crate and referenced specs
		CLIColorType::auto => {
			env::remove_var("NOCOLOR");
			env::remove_var("CLICOLOR_FORCE");
			env::remove_var("CLICOLOR");
		}
		CLIColorType::never => {
			env::set_var("NOCOLOR", "1");
			env::remove_var("CLICOLOR_FORCE");
			env::set_var("CLICOLOR", "0");
		}
		CLIColorType::always => {
			env::remove_var("NOCOLOR");
			env::set_var("CLICOLOR_FORCE", "1");
			env::remove_var("CLICOLOR");
		}
	}
	assert!(
		env::var_os("PKGDEST").is_none(),
		"PKGDEST environment is set, but RUA needs to modify it. Please run RUA without it"
	);
	let is_extension_compatible = env::var_os("PKGEXT").map_or(true, |ext| {
		let ext = ext.to_string_lossy();
		ext.ends_with(".tar") || ext.ends_with(".tar.xz")
	});
	assert!(
		is_extension_compatible,
		"PKGEXT environment is set to an incompatible value. \
		 Only *.tar and *.tar.xz are supported."
	);
	default_env("PKGEXT", ".pkg.tar.xz");

	let dirs = ProjectDirs::from("com.gitlab", "vn971", "rua")
		.expect("Failed to determine XDG directories");
	std::fs::create_dir_all(dirs.cache_dir()).expect("Failed to create project cache directory");
	rm_rf::force_remove_all(dirs.config_dir().join(".system"), true).ok();
	std::fs::create_dir_all(dirs.config_dir().join(".system"))
		.expect("Failed to create project config directory");
	std::fs::create_dir_all(dirs.config_dir().join("wrap_args.d"))
		.expect("Failed to create project config directory");
	overwrite_file(
		&dirs.config_dir().join(".system/seccomp-i686.bpf"),
		include_bytes!("../res/seccomp-i686.bpf"),
	);
	overwrite_file(
		&dirs.config_dir().join(".system/seccomp-x86_64.bpf"),
		include_bytes!("../res/seccomp-x86_64.bpf"),
	);
	let seccomp_path = format!(
		".system/seccomp-{}.bpf",
		uname::uname()
			.expect("Failed to get system architecture via uname")
			.machine
	);
	default_env(
		"RUA_SECCOMP_FILE",
		dirs.config_dir().join(seccomp_path).to_str().unwrap(),
	);
	overwrite_script(
		&dirs.config_dir().join(wrapped::WRAP_SCRIPT_PATH),
		include_bytes!("../res/wrap.sh"),
	);
	ensure_script(
		&dirs.config_dir().join(".system/wrap_args.sh.example"),
		include_bytes!("../res/wrap_args.sh"),
	);
	let locked_file = File::open(dirs.config_dir()).expect("Failed to find config dir for locking");
	locked_file.try_lock_exclusive().unwrap_or_else(|_| {
		eprintln!("Another RUA instance already running.");
		exit(2)
	});
	match config.action {
		Action::Install {
			asdeps,
			offline,
			target,
		} => {
			wrapped::install(target, &dirs, offline, asdeps);
		}
		Action::Jailbuild { offline, target } => {
			action_jailbuild::action_jailbuild(offline, target, &dirs)
		}
		Action::Search { target } => action_search::action_search(target),
		Action::Info { ref target } => {
			info(target, false).unwrap();
		}
		Action::Tarcheck { target } => {
			tar_check::tar_check(&target);
			eprintln!("Finished checking pachage: {:?}", target);
		}
	};
}
