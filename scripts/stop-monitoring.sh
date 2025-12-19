#!/bin/bash
# Stop monitoring stack

set -e

echo "🛑 Stopping Doradura Monitoring Stack..."
echo ""

docker-compose -f docker-compose.monitoring.yml down

echo ""
echo "✅ Monitoring stack stopped"
echo ""
echo "💡 To remove all data (including metrics history):"
echo "   docker-compose -f docker-compose.monitoring.yml down -v"
