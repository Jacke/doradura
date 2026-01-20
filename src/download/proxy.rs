/// Proxy management module for yt-dlp downloads
///
/// Provides:
/// - Proxy list management (loading from environment, files, URLs)
/// - Proxy selection strategies (round-robin, random, weighted)
/// - Proxy rotation and fallback
/// - Health checking for proxies
/// - Statistics and monitoring
use crate::core::error::AppError;
use rand::Rng;
use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Supported proxy protocols
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProxyProtocol {
    /// HTTP proxy
    Http,
    /// HTTPS proxy
    Https,
    /// SOCKS5 proxy
    Socks5,
}

impl fmt::Display for ProxyProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProxyProtocol::Http => write!(f, "http"),
            ProxyProtocol::Https => write!(f, "https"),
            ProxyProtocol::Socks5 => write!(f, "socks5"),
        }
    }
}

impl ProxyProtocol {
    /// Parse protocol from string
    pub fn parse_from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "http" => Some(ProxyProtocol::Http),
            "https" => Some(ProxyProtocol::Https),
            "socks5" | "socks5h" => Some(ProxyProtocol::Socks5),
            _ => None,
        }
    }
}

/// Represents a single proxy
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Proxy {
    /// Protocol (http, https, socks5)
    pub protocol: ProxyProtocol,
    /// Host (IP or hostname)
    pub host: String,
    /// Port number
    pub port: u16,
    /// Optional authentication (username:password)
    pub auth: Option<String>,
    /// Weight for weighted selection (higher = more likely)
    pub weight: u32,
}

impl Proxy {
    /// Create a new proxy
    pub fn new(protocol: ProxyProtocol, host: String, port: u16) -> Self {
        Self {
            protocol,
            host,
            port,
            auth: None,
            weight: 1,
        }
    }

    /// Create proxy with authentication
    pub fn with_auth(protocol: ProxyProtocol, host: String, port: u16, auth: String) -> Self {
        Self {
            protocol,
            host,
            port,
            auth: Some(auth),
            weight: 1,
        }
    }

    /// Set weight for proxy selection
    pub fn with_weight(mut self, weight: u32) -> Self {
        self.weight = weight.max(1); // At least 1
        self
    }

    /// Get full proxy URL for yt-dlp
    pub fn to_url(&self) -> String {
        match &self.auth {
            Some(auth) => format!("{}://{}@{}:{}", self.protocol, auth, self.host, self.port),
            None => format!("{}://{}:{}", self.protocol, self.host, self.port),
        }
    }

    /// Parse proxy from string format
    /// Formats supported:
    /// - "http://host:port"
    /// - "https://user:pass@host:port"
    /// - "socks5://host:port"
    pub fn from_string(s: &str) -> Result<Self, AppError> {
        let s = s.trim();

        // Parse protocol
        let (protocol_str, rest) = s
            .split_once("://")
            .ok_or_else(|| AppError::Download(format!("Invalid proxy format: {}", s)))?;

        let protocol = ProxyProtocol::parse_from_str(protocol_str)
            .ok_or_else(|| AppError::Download(format!("Unknown proxy protocol: {}", protocol_str)))?;

        // Parse auth and host:port
        let (auth, host_port) = if let Some(at_pos) = rest.rfind('@') {
            let auth = &rest[..at_pos];
            let host_port = &rest[at_pos + 1..];
            (Some(auth.to_string()), host_port)
        } else {
            (None, rest)
        };

        // Parse host and port
        let (host, port_str) = host_port
            .rsplit_once(':')
            .ok_or_else(|| AppError::Download(format!("Invalid proxy host:port: {}", host_port)))?;

        let port: u16 = port_str
            .parse()
            .map_err(|_| AppError::Download(format!("Invalid proxy port: {}", port_str)))?;

        Ok(Self {
            protocol,
            host: host.to_string(),
            port,
            auth,
            weight: 1,
        })
    }
}

impl fmt::Display for Proxy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_url())
    }
}

/// Proxy selection strategy
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxySelectionStrategy {
    /// Round-robin through proxies
    RoundRobin,
    /// Random selection
    Random,
    /// Weighted random selection
    Weighted,
    /// Always use the same proxy (first one)
    Fixed,
}

/// Proxy statistics
#[derive(Debug, Clone, Default)]
pub struct ProxyStats {
    /// Successful uses
    pub successes: u64,
    /// Failed uses
    pub failures: u64,
    /// Total data downloaded
    pub bytes_downloaded: u64,
}

impl fmt::Display for ProxyStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let total = self.successes + self.failures;
        let success_rate = if total > 0 {
            (self.successes as f64 / total as f64 * 100.0) as u32
        } else {
            0
        };

        write!(
            f,
            "✓: {} ✗: {} ({}%) | Downloaded: {} MB",
            self.successes,
            self.failures,
            success_rate,
            self.bytes_downloaded / 1_000_000
        )
    }
}

/// Internal proxy stats with atomic counters
struct InternalProxyStats {
    successes: AtomicU64,
    failures: AtomicU64,
    bytes_downloaded: AtomicU64,
}

impl InternalProxyStats {
    fn new() -> Self {
        Self {
            successes: AtomicU64::new(0),
            failures: AtomicU64::new(0),
            bytes_downloaded: AtomicU64::new(0),
        }
    }

    fn record_success(&self) {
        self.successes.fetch_add(1, Ordering::Relaxed);
    }

    fn record_failure(&self) {
        self.failures.fetch_add(1, Ordering::Relaxed);
    }

    fn add_bytes(&self, bytes: u64) {
        self.bytes_downloaded.fetch_add(bytes, Ordering::Relaxed);
    }

    fn to_stats(&self) -> ProxyStats {
        ProxyStats {
            successes: self.successes.load(Ordering::Relaxed),
            failures: self.failures.load(Ordering::Relaxed),
            bytes_downloaded: self.bytes_downloaded.load(Ordering::Relaxed),
        }
    }
}

/// Proxy list manager
pub struct ProxyList {
    proxies: Vec<Proxy>,
    stats: HashMap<String, Arc<InternalProxyStats>>,
    selection_strategy: ProxySelectionStrategy,
    current_index: Arc<AtomicU64>,
}

impl ProxyList {
    /// Create empty proxy list
    pub fn new(strategy: ProxySelectionStrategy) -> Self {
        Self {
            proxies: Vec::new(),
            stats: HashMap::new(),
            selection_strategy: strategy,
            current_index: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Add proxy to list
    pub fn add_proxy(&mut self, proxy: Proxy) -> Result<(), AppError> {
        if self.proxies.iter().any(|p| p.to_url() == proxy.to_url()) {
            return Err(AppError::Download(format!("Proxy already exists: {}", proxy.to_url())));
        }

        let url = proxy.to_url();
        self.proxies.push(proxy);
        self.stats.insert(url, Arc::new(InternalProxyStats::new()));
        Ok(())
    }

    /// Add proxy from string
    pub fn add_proxy_string(&mut self, proxy_str: &str) -> Result<(), AppError> {
        let proxy = Proxy::from_string(proxy_str)?;
        self.add_proxy(proxy)
    }

    /// Add multiple proxies from comma-separated list
    pub fn add_proxies_from_csv(&mut self, csv: &str) -> Result<u32, AppError> {
        let mut count = 0;
        for proxy_str in csv.split(',') {
            if self.add_proxy_string(proxy_str.trim()).is_ok() {
                count += 1;
            }
        }

        if count == 0 {
            return Err(AppError::Download("No valid proxies found in list".to_string()));
        }

        Ok(count)
    }

    /// Select next proxy based on strategy
    pub fn select(&self) -> Option<&Proxy> {
        if self.proxies.is_empty() {
            return None;
        }

        let index = match self.selection_strategy {
            ProxySelectionStrategy::RoundRobin => {
                let current = self.current_index.fetch_add(1, Ordering::Relaxed) as usize;
                current % self.proxies.len()
            }
            ProxySelectionStrategy::Random => {
                let mut rng = rand::thread_rng();
                let range = 0..self.proxies.len();
                rng.gen_range(range)
            }
            ProxySelectionStrategy::Weighted => self.select_weighted(),
            ProxySelectionStrategy::Fixed => 0,
        };

        Some(&self.proxies[index])
    }

    /// Select proxy using weighted random
    fn select_weighted(&self) -> usize {
        let mut rng = rand::thread_rng();
        let total_weight: u64 = self.proxies.iter().map(|p| p.weight as u64).sum();
        let mut selection = rng.gen_range(0..total_weight);

        for (i, proxy) in self.proxies.iter().enumerate() {
            if selection < proxy.weight as u64 {
                return i;
            }
            selection -= proxy.weight as u64;
        }

        0
    }

    /// Get proxy count
    pub fn len(&self) -> usize {
        self.proxies.len()
    }

    /// Check if proxy list is empty
    pub fn is_empty(&self) -> bool {
        self.proxies.is_empty()
    }

    /// Get all proxies
    pub fn all(&self) -> &[Proxy] {
        &self.proxies
    }

    /// Record successful use
    pub fn record_success(&mut self, proxy: &Proxy) {
        if let Some(stats) = self.stats.get(&proxy.to_url()) {
            stats.record_success();
        }
    }

    /// Record failed use
    pub fn record_failure(&mut self, proxy: &Proxy) {
        if let Some(stats) = self.stats.get(&proxy.to_url()) {
            stats.record_failure();
        }
    }

    /// Add bytes to statistics
    pub fn add_bytes(&mut self, proxy: &Proxy, bytes: u64) {
        if let Some(stats) = self.stats.get(&proxy.to_url()) {
            stats.add_bytes(bytes);
        }
    }

    /// Get statistics for proxy
    pub fn get_stats(&self, proxy: &Proxy) -> Option<ProxyStats> {
        self.stats.get(&proxy.to_url()).map(|stats| stats.to_stats())
    }

    /// Get all statistics
    pub fn all_stats(&self) -> HashMap<String, ProxyStats> {
        self.stats
            .iter()
            .map(|(url, stats)| (url.clone(), stats.to_stats()))
            .collect()
    }

    /// Get proxy health status (working rate)
    pub fn health_status(&self, proxy: &Proxy) -> f64 {
        if let Some(stats) = self.get_stats(proxy) {
            let total = stats.successes + stats.failures;
            if total > 0 {
                stats.successes as f64 / total as f64
            } else {
                1.0 // No data yet, assume healthy
            }
        } else {
            1.0
        }
    }

    /// Filter healthy proxies (above threshold)
    pub fn healthy_proxies(&self, min_rate: f64) -> Vec<&Proxy> {
        self.proxies
            .iter()
            .filter(|p| self.health_status(p) >= min_rate)
            .collect()
    }

    /// Clear all statistics
    pub fn reset_stats(&mut self) {
        self.stats.clear();
        for proxy in &self.proxies {
            self.stats.insert(proxy.to_url(), Arc::new(InternalProxyStats::new()));
        }
    }
}

impl fmt::Display for ProxyList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "ProxyList ({} proxies, strategy: {:?})",
            self.proxies.len(),
            self.selection_strategy
        )?;
        for proxy in &self.proxies {
            let stats = self
                .get_stats(proxy)
                .map(|s| s.to_string())
                .unwrap_or_else(|| "No stats".to_string());
            writeln!(f, "  - {} | {}", proxy.to_url(), stats)?;
        }
        Ok(())
    }
}

/// Thread-safe proxy list wrapper
pub struct ProxyListManager {
    list: Arc<RwLock<ProxyList>>,
}

impl ProxyListManager {
    /// Create new proxy list manager
    pub fn new(strategy: ProxySelectionStrategy) -> Self {
        Self {
            list: Arc::new(RwLock::new(ProxyList::new(strategy))),
        }
    }

    /// Add proxy
    pub async fn add_proxy(&self, proxy: Proxy) -> Result<(), AppError> {
        self.list.write().await.add_proxy(proxy)
    }

    /// Add proxy from string
    pub async fn add_proxy_string(&self, proxy_str: &str) -> Result<(), AppError> {
        self.list.write().await.add_proxy_string(proxy_str)
    }

    /// Add multiple proxies from CSV
    pub async fn add_proxies_from_csv(&self, csv: &str) -> Result<u32, AppError> {
        self.list.write().await.add_proxies_from_csv(csv)
    }

    /// Select proxy
    pub async fn select(&self) -> Option<Proxy> {
        self.list.read().await.select().cloned()
    }

    /// Record success
    pub async fn record_success(&self, proxy: &Proxy) {
        self.list.write().await.record_success(proxy)
    }

    /// Record failure
    pub async fn record_failure(&self, proxy: &Proxy) {
        self.list.write().await.record_failure(proxy)
    }

    /// Get statistics
    pub async fn get_stats(&self, proxy: &Proxy) -> Option<ProxyStats> {
        self.list.read().await.get_stats(proxy)
    }

    /// Get all statistics
    pub async fn all_stats(&self) -> HashMap<String, ProxyStats> {
        self.list.read().await.all_stats()
    }

    /// Health status
    pub async fn health_status(&self, proxy: &Proxy) -> f64 {
        self.list.read().await.health_status(proxy)
    }

    /// Get proxy count
    pub async fn len(&self) -> usize {
        self.list.read().await.len()
    }

    /// Check if empty
    pub async fn is_empty(&self) -> bool {
        self.list.read().await.is_empty()
    }

    /// Reset all proxy statistics
    pub async fn reset_stats(&self) {
        self.list.write().await.reset_stats()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proxy_creation() {
        let proxy = Proxy::new(ProxyProtocol::Http, "127.0.0.1".to_string(), 8080);
        assert_eq!(proxy.to_url(), "http://127.0.0.1:8080");
    }

    #[test]
    fn test_proxy_with_auth() {
        let proxy = Proxy::with_auth(
            ProxyProtocol::Http,
            "127.0.0.1".to_string(),
            8080,
            "user:pass".to_string(),
        );
        assert_eq!(proxy.to_url(), "http://user:pass@127.0.0.1:8080");
    }

    #[test]
    fn test_proxy_parsing() {
        let proxy = Proxy::from_string("http://127.0.0.1:8080").unwrap();
        assert_eq!(proxy.protocol, ProxyProtocol::Http);
        assert_eq!(proxy.host, "127.0.0.1");
        assert_eq!(proxy.port, 8080);
        assert_eq!(proxy.auth, None);

        let proxy_auth = Proxy::from_string("socks5://user:pass@proxy.example.com:1080").unwrap();
        assert_eq!(proxy_auth.protocol, ProxyProtocol::Socks5);
        assert_eq!(proxy_auth.auth, Some("user:pass".to_string()));
    }

    #[test]
    fn test_proxy_list() {
        let mut list = ProxyList::new(ProxySelectionStrategy::RoundRobin);
        list.add_proxy_string("http://127.0.0.1:8080").unwrap();
        list.add_proxy_string("http://127.0.0.1:8081").unwrap();

        assert_eq!(list.len(), 2);
        assert!(!list.is_empty());

        let selected = list.select().unwrap();
        assert_eq!(selected.port, 8080);
    }

    #[test]
    fn test_proxy_csv() {
        let mut list = ProxyList::new(ProxySelectionStrategy::RoundRobin);
        let count = list
            .add_proxies_from_csv("http://127.0.0.1:8080, http://127.0.0.1:8081, socks5://127.0.0.1:1080")
            .unwrap();

        assert_eq!(count, 3);
        assert_eq!(list.len(), 3);
    }

    #[test]
    fn test_proxy_stats() {
        let mut list = ProxyList::new(ProxySelectionStrategy::Fixed);
        let proxy = Proxy::new(ProxyProtocol::Http, "127.0.0.1".to_string(), 8080);
        list.add_proxy(proxy.clone()).unwrap();

        list.record_success(&proxy);
        list.record_success(&proxy);
        list.record_failure(&proxy);

        let stats = list.get_stats(&proxy).unwrap();
        assert_eq!(stats.successes, 2);
        assert_eq!(stats.failures, 1);
    }
}
