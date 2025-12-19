#!/bin/bash
# Quick start script for Prometheus + Grafana monitoring

set -e

echo "🚀 Starting Doradura Monitoring Stack..."
echo ""

# Check if Docker is running
if ! docker info > /dev/null 2>&1; then
    echo "❌ Error: Docker is not running. Please start Docker first."
    exit 1
fi

# Check if bot metrics are accessible
echo "📊 Checking bot metrics endpoint..."
if curl -s http://localhost:9094/health > /dev/null 2>&1; then
    echo "✅ Bot metrics server is running on :9094"
else
    echo "⚠️  Warning: Bot metrics server is not running on :9094"
    echo "   Make sure to:"
    echo "   1. Set METRICS_PORT=9094 in .env"
    echo "   2. Start the bot: cargo run --release"
    read -p "   Continue anyway? (y/n) " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        exit 1
    fi
fi

# Start monitoring stack
echo ""
echo "🐳 Starting Docker Compose..."
docker-compose -f docker-compose.monitoring.yml up -d

# Wait for services to be ready
echo ""
echo "⏳ Waiting for services to start..."
sleep 5

# Check Prometheus
echo ""
echo "📈 Checking Prometheus..."
if curl -s http://localhost:9091/-/healthy > /dev/null 2>&1; then
    echo "✅ Prometheus is healthy"
else
    echo "⚠️  Prometheus might not be ready yet"
fi

# Check Grafana
echo ""
echo "📊 Checking Grafana..."
if curl -s http://localhost:3000/api/health > /dev/null 2>&1; then
    echo "✅ Grafana is healthy"
else
    echo "⚠️  Grafana might not be ready yet"
fi

# Print access information
echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "✨ Monitoring Stack Started Successfully!"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "📍 Service URLs:"
echo "   • Bot Metrics:  http://localhost:9094/metrics"
echo "   • Prometheus:   http://localhost:9091"
echo "   • Grafana:      http://localhost:3000"
echo "   • AlertManager: http://localhost:9093"
echo ""
echo "🔑 Grafana Login:"
echo "   Username: admin"
echo "   Password: admin (you'll be asked to change it)"
echo ""
echo "📚 Dashboard:"
echo "   'Doradura Bot - Overview' should be available automatically"
echo ""
echo "💡 Quick Commands:"
echo "   • View logs:    docker-compose -f docker-compose.monitoring.yml logs -f"
echo "   • Stop:         docker-compose -f docker-compose.monitoring.yml down"
echo "   • Restart:      docker-compose -f docker-compose.monitoring.yml restart"
echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# Optionally open browser
if command -v open > /dev/null 2>&1; then
    read -p "🌐 Open Grafana in browser? (y/n) " -n 1 -r
    echo
    if [[ $REPLY =~ ^[Yy]$ ]]; then
        sleep 2  # Give Grafana a moment
        open http://localhost:3000
    fi
fi

echo "✅ Done! Happy monitoring! 📊"
