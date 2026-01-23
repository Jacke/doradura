//! Integration tests for proxy system

#[cfg(test)]
mod proxy_tests {
    use doradura::download::{Proxy, ProxyListManager, ProxyProtocol, ProxySelectionStrategy};
    use std::sync::Arc;

    // ============================================================================
    // ProxyListManager async tests
    // ============================================================================

    #[tokio::test]
    async fn test_proxy_list_manager_creation() {
        let manager = ProxyListManager::new(ProxySelectionStrategy::RoundRobin);
        assert!(manager.is_empty().await);
        assert_eq!(manager.len().await, 0);
    }

    #[tokio::test]
    async fn test_proxy_list_manager_add_proxy() {
        let manager = ProxyListManager::new(ProxySelectionStrategy::RoundRobin);

        let proxy = Proxy::new(ProxyProtocol::Http, "127.0.0.1".to_string(), 8080);
        manager.add_proxy(proxy.clone()).await.unwrap();

        assert!(!manager.is_empty().await);
        assert_eq!(manager.len().await, 1);
    }

    #[tokio::test]
    async fn test_proxy_list_manager_select() {
        let manager = ProxyListManager::new(ProxySelectionStrategy::RoundRobin);

        let proxy1 = Proxy::new(ProxyProtocol::Http, "proxy1.com".to_string(), 8080);
        let proxy2 = Proxy::new(ProxyProtocol::Http, "proxy2.com".to_string(), 8081);

        manager.add_proxy(proxy1.clone()).await.unwrap();
        manager.add_proxy(proxy2.clone()).await.unwrap();

        let selected = manager.select().await.unwrap();
        assert_eq!(selected.host, "proxy1.com");

        let selected = manager.select().await.unwrap();
        assert_eq!(selected.host, "proxy2.com");
    }

    #[tokio::test]
    async fn test_proxy_list_manager_record_success() {
        let manager = ProxyListManager::new(ProxySelectionStrategy::Fixed);

        let proxy = Proxy::new(ProxyProtocol::Http, "127.0.0.1".to_string(), 8080);
        manager.add_proxy(proxy.clone()).await.unwrap();

        for _ in 0..5 {
            manager.record_success(&proxy).await;
        }

        let stats = manager.all_stats().await;
        let proxy_stats = stats.get("http://127.0.0.1:8080").unwrap();
        assert_eq!(proxy_stats.successes, 5);
    }

    #[tokio::test]
    async fn test_proxy_list_manager_record_failure() {
        let manager = ProxyListManager::new(ProxySelectionStrategy::Fixed);

        let proxy = Proxy::new(ProxyProtocol::Http, "127.0.0.1".to_string(), 8080);
        manager.add_proxy(proxy.clone()).await.unwrap();

        for _ in 0..3 {
            manager.record_failure(&proxy).await;
        }

        let stats = manager.all_stats().await;
        let proxy_stats = stats.get("http://127.0.0.1:8080").unwrap();
        assert_eq!(proxy_stats.failures, 3);
    }

    #[tokio::test]
    async fn test_proxy_list_manager_health_status() {
        let manager = ProxyListManager::new(ProxySelectionStrategy::Fixed);

        let proxy = Proxy::new(ProxyProtocol::Http, "127.0.0.1".to_string(), 8080);
        manager.add_proxy(proxy.clone()).await.unwrap();

        // Record 8 successes and 2 failures = 80% health
        for _ in 0..8 {
            manager.record_success(&proxy).await;
        }
        for _ in 0..2 {
            manager.record_failure(&proxy).await;
        }

        let health = manager.health_status(&proxy).await;
        assert_eq!(health, 0.8);
    }

    #[tokio::test]
    async fn test_proxy_list_manager_reset_stats() {
        let manager = ProxyListManager::new(ProxySelectionStrategy::Fixed);

        let proxy = Proxy::new(ProxyProtocol::Http, "127.0.0.1".to_string(), 8080);
        manager.add_proxy(proxy.clone()).await.unwrap();

        for _ in 0..5 {
            manager.record_success(&proxy).await;
        }
        for _ in 0..3 {
            manager.record_failure(&proxy).await;
        }

        let stats_before = manager.all_stats().await;
        let proxy_stats = stats_before.get("http://127.0.0.1:8080").unwrap();
        assert_eq!(proxy_stats.successes, 5);
        assert_eq!(proxy_stats.failures, 3);

        // Reset stats
        manager.reset_stats().await;

        let stats_after = manager.all_stats().await;
        let proxy_stats = stats_after.get("http://127.0.0.1:8080").unwrap();
        assert_eq!(proxy_stats.successes, 0);
        assert_eq!(proxy_stats.failures, 0);
    }

    #[tokio::test]
    async fn test_proxy_list_manager_all_strategies() {
        let strategies = vec![
            ProxySelectionStrategy::RoundRobin,
            ProxySelectionStrategy::Random,
            ProxySelectionStrategy::Weighted,
            ProxySelectionStrategy::Fixed,
        ];

        for strategy in strategies {
            let manager = ProxyListManager::new(strategy);

            let proxy1 = Proxy::new(ProxyProtocol::Http, "proxy1.com".to_string(), 8080);
            let proxy2 = Proxy::new(ProxyProtocol::Http, "proxy2.com".to_string(), 8081);

            manager.add_proxy(proxy1.clone()).await.unwrap();
            manager.add_proxy(proxy2.clone()).await.unwrap();

            // Should always have 2 proxies regardless of strategy
            assert_eq!(manager.len().await, 2);

            // Select should return Some for non-empty list
            let selected = manager.select().await;
            assert!(selected.is_some());
        }
    }

    #[tokio::test]
    async fn test_proxy_list_manager_add_from_csv_async() {
        let manager = ProxyListManager::new(ProxySelectionStrategy::RoundRobin);

        let csv = "http://proxy1.com:8080, https://proxy2.com:8443, socks5://proxy3.com:1080";
        let count = manager.add_proxies_from_csv(csv).await.unwrap();

        assert_eq!(count, 3);
        assert_eq!(manager.len().await, 3);
    }

    #[tokio::test]
    async fn test_proxy_list_manager_empty_selection() {
        let manager = ProxyListManager::new(ProxySelectionStrategy::RoundRobin);

        // Empty manager should return None on select
        assert!(manager.select().await.is_none());
    }

    // ============================================================================
    // Concurrent access tests
    // ============================================================================

    #[tokio::test]
    async fn test_proxy_manager_concurrent_access() {
        let manager = ProxyListManager::new(ProxySelectionStrategy::RoundRobin);

        let proxy = Proxy::new(ProxyProtocol::Http, "127.0.0.1".to_string(), 8080);
        manager.add_proxy(proxy.clone()).await.unwrap();

        let manager: Arc<ProxyListManager> = Arc::new(manager);

        // Spawn multiple concurrent tasks
        let mut handles = vec![];

        for i in 0..10 {
            let proxy_clone = proxy.clone();
            let manager_clone: Arc<ProxyListManager> = Arc::clone(&manager);

            let handle = tokio::spawn(async move {
                if i % 2 == 0 {
                    manager_clone.record_success(&proxy_clone).await;
                } else {
                    manager_clone.record_failure(&proxy_clone).await;
                }
            });

            handles.push(handle);
        }

        // Wait for all tasks
        for handle in handles {
            let _ = handle.await;
        }

        // Verify stats
        let stats = manager.all_stats().await;
        let proxy_stats = stats.get("http://127.0.0.1:8080").unwrap();

        assert_eq!(proxy_stats.successes, 5);
        assert_eq!(proxy_stats.failures, 5);
    }

    #[tokio::test]
    async fn test_proxy_manager_concurrent_selection() {
        let manager = ProxyListManager::new(ProxySelectionStrategy::RoundRobin);

        let proxy1 = Proxy::new(ProxyProtocol::Http, "proxy1.com".to_string(), 8080);
        let proxy2 = Proxy::new(ProxyProtocol::Http, "proxy2.com".to_string(), 8081);

        manager.add_proxy(proxy1.clone()).await.unwrap();
        manager.add_proxy(proxy2.clone()).await.unwrap();

        let manager: Arc<ProxyListManager> = Arc::new(manager);

        // Multiple concurrent selections should work
        let mut handles = vec![];

        for _ in 0..20 {
            let manager_clone: Arc<ProxyListManager> = Arc::clone(&manager);
            let handle = tokio::spawn(async move {
                let selected = manager_clone.select().await;
                assert!(selected.is_some());
            });
            handles.push(handle);
        }

        for handle in handles {
            let _ = handle.await;
        }
    }

    // ============================================================================
    // Edge case tests
    // ============================================================================

    #[tokio::test]
    async fn test_proxy_with_authentication() {
        let manager = ProxyListManager::new(ProxySelectionStrategy::Fixed);

        let proxy = Proxy::with_auth(
            ProxyProtocol::Socks5,
            "proxy.example.com".to_string(),
            1080,
            "user:password".to_string(),
        );

        manager.add_proxy(proxy.clone()).await.unwrap();

        let selected = manager.select().await.unwrap();
        assert_eq!(selected.auth, Some("user:password".to_string()));
        assert_eq!(selected.to_url(), "socks5://user:password@proxy.example.com:1080");
    }

    #[tokio::test]
    async fn test_proxy_different_protocols() {
        let manager = ProxyListManager::new(ProxySelectionStrategy::RoundRobin);

        let http_proxy = Proxy::new(ProxyProtocol::Http, "proxy1.com".to_string(), 8080);
        let https_proxy = Proxy::new(ProxyProtocol::Https, "proxy2.com".to_string(), 8443);
        let socks_proxy = Proxy::new(ProxyProtocol::Socks5, "proxy3.com".to_string(), 1080);

        manager.add_proxy(http_proxy.clone()).await.unwrap();
        manager.add_proxy(https_proxy.clone()).await.unwrap();
        manager.add_proxy(socks_proxy.clone()).await.unwrap();

        assert_eq!(manager.len().await, 3);

        // Verify all proxies are returned correctly
        let selected1 = manager.select().await.unwrap();
        assert_eq!(selected1.protocol, ProxyProtocol::Http);

        let selected2 = manager.select().await.unwrap();
        assert_eq!(selected2.protocol, ProxyProtocol::Https);

        let selected3 = manager.select().await.unwrap();
        assert_eq!(selected3.protocol, ProxyProtocol::Socks5);
    }

    #[tokio::test]
    async fn test_large_proxy_list() {
        let manager = ProxyListManager::new(ProxySelectionStrategy::RoundRobin);

        // Add 100 proxies
        for i in 0..100 {
            let proxy = Proxy::new(ProxyProtocol::Http, format!("proxy{}.com", i), 8000 + i as u16);
            manager.add_proxy(proxy).await.unwrap();
        }

        assert_eq!(manager.len().await, 100);

        // All should be selectable
        for _ in 0..100 {
            let selected = manager.select().await;
            assert!(selected.is_some());
        }
    }

    #[test]
    fn test_proxy_url_formatting() {
        let proxy_http = Proxy::new(ProxyProtocol::Http, "proxy.example.com".to_string(), 8080);
        assert_eq!(proxy_http.to_url(), "http://proxy.example.com:8080");

        let proxy_https = Proxy::new(ProxyProtocol::Https, "secure.proxy.com".to_string(), 8443);
        assert_eq!(proxy_https.to_url(), "https://secure.proxy.com:8443");

        let proxy_socks = Proxy::new(ProxyProtocol::Socks5, "socks.proxy.com".to_string(), 1080);
        assert_eq!(proxy_socks.to_url(), "socks5://socks.proxy.com:1080");
    }

    #[test]
    fn test_proxy_display_format() {
        let proxy = Proxy::new(ProxyProtocol::Http, "127.0.0.1".to_string(), 8080);
        let display_str = format!("{}", proxy);
        assert!(display_str.contains("http://127.0.0.1:8080"));
    }
}
