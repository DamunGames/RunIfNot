// リリースビルド時のみウィンドウを表示しない
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::{Deserialize, Serialize};
use sysinfo::{ProcessRefreshKind, RefreshKind, System};
use std::path::Path;
use std::fs;
use std::collections::HashSet;
use std::process::Command;
use std::thread;
use std::time::Duration;

/// 設定ファイルの構造を定義
#[derive(Debug, Deserialize, Serialize)]
struct Config {
    processes: ProcessConfig,
    execution: ExecutionConfig,
}

/// プロセス監視に関する設定
#[derive(Debug, Deserialize, Serialize)]
struct ProcessConfig {
    /// 監視対象のプロセス名リスト
    observe_names: Vec<String>,
    /// 監視対象が起動時に終了させるプロセス名
    executable_name: String,
}

/// 実行に関する設定
#[derive(Debug, Deserialize, Serialize)]
struct ExecutionConfig {
    /// 監視間隔（秒）
    interval_seconds: u64,
    /// 起動するコマンド
    command: String,
    /// コマンドに渡す引数
    arguments: Vec<String>,
}

/// デフォルトの監視対象プロセス名を生成
fn default_observe_names() -> Vec<String> {
    vec![
        "taskmgr.exe".to_string(),
        "mspaint.exe".to_string(),
    ]
}

/// 設定のデフォルト値を定義
impl Default for Config {
    fn default() -> Self {
        Config {
            processes: ProcessConfig {
                observe_names: default_observe_names(),
                executable_name: "calculatorapp.exe".to_string(),
            },
            execution: ExecutionConfig {
                interval_seconds: 5,
                command: "cmd".to_string(),
                arguments: vec!["/C".to_string(), "start".to_string(), "".to_string(), "C:/Windows/System32/calc.exe".to_string()],
            },
        }
    }
}

/// 設定ファイルのパスを取得
/// 優先順位: 実行ファイルのあるディレクトリ > カレントディレクトリ
fn get_config_path() -> String {
    // 実行ファイルのあるディレクトリを取得
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let config_path = exe_dir.join("config.toml");
            if config_path.exists() {
                return config_path.to_string_lossy().to_string();
            }
        }
    }

    // カレントディレクトリを取得
    "config.toml".to_string()
}

/// 設定ファイルを読み込む。存在しない場合はデフォルト設定で作成
fn load_config() -> Result<Config, Box<dyn std::error::Error>> {
    let config_path = get_config_path(); // 設定ファイルのパス

    // 設定ファイルが存在しない場合、デフォルト設定で作成
    if !Path::new(&config_path).exists() {
        let default_config = Config::default();
        let toml_str = toml::to_string_pretty(&default_config)?;
        println!("デフォルトの設定ファイルを作成します: {}", config_path);
        fs::write(config_path, toml_str)?;
        return Ok(default_config);
    }

    // 設定ファイルを読み込んでパース
    let config_content = fs::read_to_string(config_path)?;
    let config: Config = toml::from_str(&config_content)?;

    // lowercase変換して重複を排除
    let mut observe_set = HashSet::new();
    for name in config.processes.observe_names {
        observe_set.insert(name.to_lowercase());
    }
    let deduped_observe_names: Vec<String> = observe_set.into_iter().collect();

    let config = Config {
        processes: ProcessConfig {
            observe_names: deduped_observe_names,
            executable_name: config.processes.executable_name.to_lowercase(),
        },
        execution: config.execution,
    };

    Ok(config)
}

/// 新しいプロセスを起動する
#[inline(always)]
fn start_process(command: &str, arguments: &[String]) {
    match Command::new(command)
        .args(arguments)
        .spawn()
    {
        Ok(child) => {
            println!("プロセスを起動しました: {}", child.id());
        },
        Err(e) => {
            println!("プロセスの起動に失敗しました: {}", e);
        },
    }
}

/// 設定で指定された監視対象プロセス（observe_names + executable_name）のPIDを取得
#[inline(always)]
fn get_target_process_pids(sys: &System, config: &Config) -> HashSet<sysinfo::Pid> {
    sys.processes()
        .iter()
        .filter(|(_, proc_)| {
            // 監視対象プロセスまたは実行可能プロセスに一致するかチェック
            config.processes.observe_names.iter().any(|name| proc_.name().eq_ignore_ascii_case(name))
            || proc_.name().eq_ignore_ascii_case(&config.processes.executable_name)
        })
        .map(|(pid, _)| *pid)
        .collect()
}

/// プロセス名が監視対象に含まれるかチェック
#[inline(always)]
fn is_observed_process(process_name: &str, observe_names: &[String]) -> bool {
    observe_names.iter().any(|name| process_name.eq_ignore_ascii_case(name))
}

/// executable_nameで指定されたプロセスを終了
#[inline(always)]
fn terminate_executable_process(sys: &System, config: &Config) {
    if let Some((pid, process)) = sys.processes()
        .iter()
        .find(|(_, p)| p.name().eq_ignore_ascii_case(&config.processes.executable_name)) 
    {
        println!("{} を終了します (PID: {})", &config.processes.executable_name, pid);
        
        if !process.kill() {
            eprintln!("{} の終了に失敗しました", &config.processes.executable_name);
        }
    } else {
        println!("{} は起動していません", &config.processes.executable_name);
    }
}

/// 監視対象プロセスが存在しない場合の処理
#[inline(always)]
fn handle_no_processes(config: &Config) {
    println!("プロセスが存在しません。実行可能ファイルを起動します。");
    start_process(&config.execution.command, &config.execution.arguments);
}

/// 新しいプロセスが検出された場合の処理
#[inline(always)]
fn handle_new_processes(sys: &System, config: &Config, new_pids: &HashSet<sysinfo::Pid>) {
    for pid in new_pids {
        if let Some(process) = sys.process(*pid) {
            // 新しく起動したプロセスが監視対象かチェック
            if is_observed_process(process.name(), &config.processes.observe_names) {
                println!("新しいプロセスが起動しました: {} (PID: {})", process.name(), pid);
                terminate_executable_process(sys, config);
            }
        }
    }
}

fn main() {
    // 設定ファイルを読み込む
    match load_config() {
        Ok(config) => {
            println!("設定読み込み完了: {:?}", config);

            // システム情報を初期化（プロセス情報のみ取得）
            let mut sys = System::new_with_specifics(
                RefreshKind::new().with_processes(ProcessRefreshKind::everything())
            );
            
            // 初回のプロセス情報を取得
            sys.refresh_processes();
            let mut known_pids = get_target_process_pids(&sys, &config);

            // 起動時に監視対象プロセスが存在しない場合、実行可能ファイルを起動
            if known_pids.is_empty() {
                handle_no_processes(&config);
            }

            // メインループ：定期的にプロセス状態を監視
            loop {
                // 設定された間隔で待機
                thread::sleep(Duration::from_secs(config.execution.interval_seconds));

                // プロセス情報を更新
                sys.refresh_processes();
                let current_pids = get_target_process_pids(&sys, &config);

                // 監視対象のプロセスが一つも存在しない場合、実行可能ファイルを起動
                if current_pids.is_empty() {
                    handle_no_processes(&config);
                } else {
                    // 新しく起動したプロセスを取得
                    let new_pids: HashSet<_> = current_pids.difference(&known_pids).copied().collect();
                    
                    if !new_pids.is_empty() {
                        handle_new_processes(&sys, &config, &new_pids);
                    }
                }

                // 次回比較用にPIDリストを更新
                known_pids = current_pids;
            }
        }
        Err(e) => {
            eprintln!("設定の読み込みに失敗しました: {}", e);
            std::process::exit(1);
        }
    }
}