//! Smoke test configuration and runner.
//!
//! Provides configuration for running smoke tests in different environments:
//! - CI: Full parallel tests with residential proxy allowed
//! - Production: Sequential tests without residential proxy

use super::results::SmokeTestReport;
use super::test_cases::{
    test_audio_download, test_cookies_validation, test_ffmpeg_toolchain, test_metadata_extraction, test_video_download,
};
use super::{DEFAULT_TEST_TIMEOUT_SECS, DEFAULT_TEST_URL, PRODUCTION_TEST_TIMEOUT_SECS};
use crate::download::metadata::ProxyConfig;
use std::time::{Duration, Instant};

/// Configuration for smoke test execution
#[derive(Debug, Clone)]
pub struct SmokeTestConfig {
    /// Test URL to use for downloads
    pub test_url: String,
    /// Timeout for individual tests
    pub timeout: Duration,
    /// Whether to run tests in parallel
    pub parallel: bool,
    /// Whether to allow residential proxy for tests
    pub allow_residential_proxy: bool,
    /// Temporary directory for downloaded files
    pub temp_dir: String,
}

impl Default for SmokeTestConfig {
    fn default() -> Self {
        Self {
            test_url: DEFAULT_TEST_URL.to_string(),
            timeout: Duration::from_secs(DEFAULT_TEST_TIMEOUT_SECS),
            parallel: false,
            allow_residential_proxy: true,
            temp_dir: std::env::temp_dir().to_string_lossy().to_string(),
        }
    }
}

impl SmokeTestConfig {
    /// Creates configuration for CI environment.
    ///
    /// - Longer timeout (180s)
    /// - Parallel execution enabled
    /// - Residential proxy allowed
    pub fn for_ci() -> Self {
        Self {
            test_url: DEFAULT_TEST_URL.to_string(),
            timeout: Duration::from_secs(DEFAULT_TEST_TIMEOUT_SECS),
            parallel: true,
            allow_residential_proxy: true,
            temp_dir: std::env::temp_dir().to_string_lossy().to_string(),
        }
    }

    /// Creates configuration for production health checks.
    ///
    /// - Shorter timeout (120s)
    /// - Sequential execution (less load)
    /// - NO residential proxy (avoid costs)
    pub fn for_production() -> Self {
        Self {
            test_url: DEFAULT_TEST_URL.to_string(),
            timeout: Duration::from_secs(PRODUCTION_TEST_TIMEOUT_SECS),
            parallel: false,
            allow_residential_proxy: false,
            temp_dir: std::env::temp_dir().to_string_lossy().to_string(),
        }
    }

    /// Creates a custom configuration.
    pub fn custom(test_url: &str, timeout_secs: u64, parallel: bool, allow_residential: bool) -> Self {
        Self {
            test_url: test_url.to_string(),
            timeout: Duration::from_secs(timeout_secs),
            parallel,
            allow_residential_proxy: allow_residential,
            temp_dir: std::env::temp_dir().to_string_lossy().to_string(),
        }
    }
}

/// Returns proxy chain for smoke tests based on configuration.
///
/// # Arguments
///
/// * `allow_residential` - Whether to include residential proxy in the chain
///
/// # Returns
///
/// List of proxy configurations to try in order:
/// 1. WARP (always)
/// 2. Residential (only if allow_residential=true)
/// 3. Direct (fallback)
pub fn get_smoke_test_proxy_chain(allow_residential: bool) -> Vec<Option<ProxyConfig>> {
    use crate::core::config;

    let mut chain = Vec::new();

    // Primary: WARP proxy (free Cloudflare)
    if let Some(ref warp_proxy) = *config::proxy::WARP_PROXY {
        if !warp_proxy.trim().is_empty() {
            chain.push(Some(ProxyConfig::new(
                warp_proxy.trim().to_string(),
                "WARP (Cloudflare)",
            )));
        }
    }

    // Fallback: Residential proxy (only if allowed)
    if allow_residential {
        if let Some(ref proxy_list) = *config::proxy::PROXY_LIST {
            let first_proxy = proxy_list.split(',').next().unwrap_or("").trim();
            if !first_proxy.is_empty() {
                chain.push(Some(ProxyConfig::new(
                    first_proxy.to_string(),
                    "Residential (fallback)",
                )));
            }
        }
    }

    // Last resort: No proxy (direct connection)
    chain.push(None);

    chain
}

/// Runs all smoke tests and returns a report.
///
/// # Arguments
///
/// * `config` - Test configuration
///
/// # Returns
///
/// A report containing results for all tests
pub async fn run_all_smoke_tests(config: &SmokeTestConfig) -> SmokeTestReport {
    let start = Instant::now();
    let mut results = Vec::new();

    log::info!("Starting smoke tests with config: {:?}", config);

    // Test 1: FFmpeg toolchain
    log::info!("Running test: ffmpeg_toolchain");
    let result = test_ffmpeg_toolchain().await;
    log::info!("  Result: {:?}", result.status);
    results.push(result);

    // Test 2: Cookies validation
    log::info!("Running test: cookies_validation");
    let result = test_cookies_validation().await;
    log::info!("  Result: {:?}", result.status);
    results.push(result);

    // Get proxy chain for download tests (NO cookies used - see test_cases.rs)
    let proxy_chain = get_smoke_test_proxy_chain(config.allow_residential_proxy);

    // Test 3: Metadata extraction
    log::info!("Running test: metadata_extraction");
    let result = test_metadata_extraction(&config.test_url, config.timeout, &proxy_chain).await;
    log::info!("  Result: {:?}", result.status);
    results.push(result);

    // Test 4: Audio download
    log::info!("Running test: audio_download");
    let result = test_audio_download(&config.test_url, &config.temp_dir, config.timeout, &proxy_chain).await;
    log::info!("  Result: {:?}", result.status);
    results.push(result);

    // Test 5: Video download
    log::info!("Running test: video_download");
    let result = test_video_download(&config.test_url, &config.temp_dir, config.timeout, &proxy_chain).await;
    log::info!("  Result: {:?}", result.status);
    results.push(result);

    let total_duration = start.elapsed();
    let report = SmokeTestReport::new(results, total_duration);

    log::info!("Smoke tests completed: {}", report.format_log());

    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_for_ci() {
        let config = SmokeTestConfig::for_ci();
        assert!(config.parallel);
        assert!(config.allow_residential_proxy);
        assert_eq!(config.timeout, Duration::from_secs(180));
    }

    #[test]
    fn test_config_for_production() {
        let config = SmokeTestConfig::for_production();
        assert!(!config.parallel);
        assert!(!config.allow_residential_proxy);
        assert_eq!(config.timeout, Duration::from_secs(120));
    }

    #[test]
    fn test_proxy_chain_with_residential() {
        let chain = get_smoke_test_proxy_chain(true);
        // Should have at least direct connection
        assert!(!chain.is_empty());
        // Last should be None (direct)
        assert!(chain.last().unwrap().is_none());
    }

    #[test]
    fn test_proxy_chain_without_residential() {
        let chain = get_smoke_test_proxy_chain(false);
        // Should have at least direct connection
        assert!(!chain.is_empty());
        // Last should be None (direct)
        assert!(chain.last().unwrap().is_none());
    }
}
