use crate::config::Config;

pub use crate::discover::registry::{
    classify_command, has_rtk_disabled_prefix, rewrite_command, split_command_chain,
    strip_disabled_prefix, Classification,
};

/// Rewrite a shell command using the exclusions configured for RTK.
pub fn rewrite_with_config(command: &str, config: &Config) -> Option<String> {
    rewrite_command(command, &config.hooks.exclude_commands)
}

/// Rewrite a shell command using the default RTK config from disk.
pub fn rewrite_with_default_config(command: &str) -> anyhow::Result<Option<String>> {
    Ok(rewrite_with_config(command, &Config::load()?))
}
