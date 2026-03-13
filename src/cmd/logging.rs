use anyhow::Result;
use flashcron::Config;
use log::LevelFilter;

use super::args::Cli;

/// Initialize logging using fern and log
pub fn init_logging(cli: &Cli, config_path: &std::path::PathBuf) -> Result<()> {
    // Load config to get settings, fallback to default if not found
    let settings = if let Ok(config) = Config::from_file(config_path) {
        config.settings
    } else {
        flashcron::config::Settings::default()
    };

    let tz = settings.effective_timezone();

    // Priority: CLI > Config
    let log_level = cli.log_level.as_ref().unwrap_or(&settings.log_level);
    let use_json = cli.json || settings.json_logs;

    let level = match log_level.to_lowercase().as_str() {
        "trace" => LevelFilter::Trace,
        "debug" => LevelFilter::Debug,
        "info" => LevelFilter::Info,
        "warn" => LevelFilter::Warn,
        "error" => LevelFilter::Error,
        _ => LevelFilter::Info,
    };

    let mut base_config = fern::Dispatch::new()
        .level(level)
        .level_for("tokio_util", LevelFilter::Warn)
        .level_for("hyper", LevelFilter::Warn);

    if use_json {
        base_config = base_config.chain(
            fern::Dispatch::new()
                .format(move |out, message, record| {
                    let timestamp = chrono::Utc::now()
                        .with_timezone(&tz)
                        .format("%Y-%m-%d %H:%M:%S%.3f")
                        .to_string();

                    // Extract KV pairs from the record
                    let mut kv_map = serde_json::Map::new();

                    struct JsonVisitor<'a>(&'a mut serde_json::Map<String, serde_json::Value>);
                    impl<'kvs> log::kv::Visitor<'kvs> for JsonVisitor<'_> {
                        fn visit_pair(
                            &mut self,
                            key: log::kv::Key<'kvs>,
                            value: log::kv::Value<'kvs>,
                        ) -> std::result::Result<(), log::kv::Error> {
                            let key_str = key.to_string();
                            // Standardize job_name to job
                            let final_key = if key_str == "job_name" {
                                "job"
                            } else {
                                &key_str
                            };
                            self.0.insert(
                                final_key.to_string(),
                                serde_json::Value::String(value.to_string()),
                            );
                            Ok(())
                        }
                    }
                    let _ = record.key_values().visit(&mut JsonVisitor(&mut kv_map));

                    let mut json_obj = serde_json::Map::new();
                    json_obj.insert(
                        "timestamp".to_string(),
                        serde_json::Value::String(timestamp),
                    );
                    json_obj.insert(
                        "level".to_string(),
                        serde_json::Value::String(record.level().to_string()),
                    );

                    let msg_str = message.to_string();

                    let mut content = serde_json::Map::new();
                    for (k, v) in kv_map {
                        content.insert(k, v);
                    }
                    if !msg_str.is_empty() {
                        content.insert("msg".to_string(), serde_json::Value::String(msg_str));
                    }

                    if content.is_empty() {
                        json_obj.insert(
                            "message".to_string(),
                            serde_json::Value::String(String::new()),
                        );
                    } else {
                        json_obj.insert("message".to_string(), serde_json::Value::Object(content));
                    }

                    out.finish(format_args!("{}", serde_json::Value::Object(json_obj)));
                })
                .chain(std::io::stdout()),
        );
    } else {
        base_config = base_config.chain(
            fern::Dispatch::new()
                .format(move |out, message, record| {
                    let timestamp = chrono::Utc::now()
                        .with_timezone(&tz)
                        .format("%Y-%m-%d %H:%M:%S%.3f")
                        .to_string();

                    let mut kvs = Vec::new();
                    struct TextVisitor<'a>(&'a mut Vec<(String, String)>);
                    impl<'kvs> log::kv::Visitor<'kvs> for TextVisitor<'_> {
                        fn visit_pair(
                            &mut self,
                            key: log::kv::Key<'kvs>,
                            value: log::kv::Value<'kvs>,
                        ) -> std::result::Result<(), log::kv::Error> {
                            self.0.push((key.to_string(), value.to_string()));
                            Ok(())
                        }
                    }
                    let _ = record.key_values().visit(&mut TextVisitor(&mut kvs));

                    let msg_str = message.to_string();
                    let mut final_msg = String::new();

                    for (k, v) in kvs {
                        if !final_msg.is_empty() {
                            final_msg.push(' ');
                        }
                        final_msg.push_str(&format!("{}={}", k, v));
                    }

                    if !msg_str.is_empty() {
                        if !final_msg.is_empty() {
                            final_msg.push(' ');
                        }
                        final_msg.push_str(&msg_str);
                    }

                    out.finish(format_args!(
                        "{}  {:<5} {}",
                        timestamp,
                        record.level(),
                        final_msg
                    ))
                })
                .chain(std::io::stdout()),
        );
    }

    base_config
        .apply()
        .map_err(|e| anyhow::anyhow!("Failed to initialize logging: {}", e))?;

    Ok(())
}
