use anyhow::Result;

pub fn run() -> Result<i32> {
    let cwd = std::env::current_dir()?;
    println!("{}", cwd.display());
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pwd_returns_success() {
        assert_eq!(run().unwrap(), 0);
    }
}
