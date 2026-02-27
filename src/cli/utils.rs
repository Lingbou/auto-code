pub fn split_command_tokens(input: &str) -> Result<Vec<String>, String> {
    shell_words::split(input).map_err(|err| format!("failed to parse command arguments: {}", err))
}
