#!/bin/bash
# Check metrics and monitoring health

set -e

echo "🔍 Doradura Monitoring Health Check"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# Check bot metrics
echo "📊 Bot Metrics Server:"
if curl -s http://localhost:9094/health > /dev/null 2>&1; then
    echo "   ✅ Running on :9094"
    HEALTH=$(curl -s http://localhost:9094/health)
    echo "   Status: $(echo $HEALTH | jq -r .status)"
    echo "   Uptime: $(echo $HEALTH | jq -r .uptime_human)"
else
    echo "   ❌ Not running on :9094"
    echo "   💡 Check that bot is started with METRICS_PORT=9094"
fi

echo ""

# Check Prometheus
echo "📈 Prometheus:"
if curl -s http://localhost:9091/-/healthy > /dev/null 2>&1; then
    echo "   ✅ Running on :9091"

    # Check targets
    TARGETS=$(curl -s http://localhost:9091/api/v1/targets | jq -r '.data.activeTargets[] | select(.labels.job=="doradura-bot") | .health')
    if [ "$TARGETS" == "up" ]; then
        echo "   ✅ Bot metrics target is UP"
    else
        echo "   ⚠️  Bot metrics target is DOWN"
    fi
else
    echo "   ❌ Not running"
fi

echo ""

# Check Grafana
echo "📊 Grafana:"
if curl -s http://localhost:3000/api/health > /dev/null 2>&1; then
    echo "   ✅ Running on :3000"

    # Check datasource
    DS_HEALTH=$(curl -s -u admin:admin http://localhost:3000/api/datasources/1/health 2>/dev/null || echo '{"status":"error"}')
    DS_STATUS=$(echo $DS_HEALTH | jq -r '.status // "unknown"')

    if [ "$DS_STATUS" == "ok" ]; then
        echo "   ✅ Prometheus datasource connected"
    else
        echo "   ⚠️  Prometheus datasource not connected"
    fi
else
    echo "   ❌ Not running"
fi

echo ""

# Check AlertManager
echo "🔔 AlertManager:"
if curl -s http://localhost:9093/-/healthy > /dev/null 2>&1; then
    echo "   ✅ Running on :9093"

    # Check active alerts
    ALERTS=$(curl -s http://localhost:9093/api/v1/alerts | jq '[.data[] | select(.status.state=="active")] | length')
    if [ "$ALERTS" -gt 0 ]; then
        echo "   ⚠️  $ALERTS active alert(s)"
    else
        echo "   ✅ No active alerts"
    fi
else
    echo "   ❌ Not running"
fi

echo ""

# Sample metrics
echo "📈 Sample Metrics (last 5 minutes):"
if curl -s http://localhost:9091/api/v1/query > /dev/null 2>&1; then

    # Total downloads
    DOWNLOADS=$(curl -s "http://localhost:9091/api/v1/query?query=increase(doradura_download_success_total\[5m\])" | jq -r '.data.result[0].value[1] // "0"' | cut -d. -f1)
    echo "   Downloads: $DOWNLOADS"

    # Success rate
    SUCCESS_RATE=$(curl -s "http://localhost:9091/api/v1/query?query=sum(rate(doradura_download_success_total\[5m\]))/(sum(rate(doradura_download_success_total\[5m\]))+sum(rate(doradura_download_failure_total\[5m\])))*100" | jq -r '.data.result[0].value[1] // "N/A"')
    if [ "$SUCCESS_RATE" != "N/A" ]; then
        SUCCESS_RATE=$(printf "%.1f%%" $SUCCESS_RATE)
    fi
    echo "   Success Rate: $SUCCESS_RATE"

    # Queue depth
    QUEUE=$(curl -s "http://localhost:9091/api/v1/query?query=doradura_queue_depth" | jq -r '.data.result[0].value[1] // "0"' | cut -d. -f1)
    echo "   Queue Depth: $QUEUE"

    # DAU
    DAU=$(curl -s "http://localhost:9091/api/v1/query?query=doradura_daily_active_users" | jq -r '.data.result[0].value[1] // "0"' | cut -d. -f1)
    echo "   Daily Active Users: $DAU"
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "✅ Health check complete!"
echo ""
echo "💡 View full metrics: http://localhost:9090/metrics"
echo "💡 View Prometheus: http://localhost:9091"
echo "💡 View Grafana: http://localhost:3000"
