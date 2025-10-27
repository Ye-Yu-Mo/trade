#!/bin/bash

# 日志查看工具

echo "============================================================"
echo "  交易日志查看"
echo "============================================================"
echo ""

# 检查 jq 是否安装
if ! command -v jq &> /dev/null; then
    echo "⚠️  建议安装 jq 以获得更好的日志查看体验"
    echo "   macOS: brew install jq"
    echo "   Ubuntu: sudo apt install jq"
    echo ""
fi

# 检查日志目录
if [ ! -d logs ]; then
    echo "❌ logs/ 目录不存在，程序可能尚未运行"
    exit 1
fi

echo "选择查看选项："
echo "  1) 最近10条交易记录"
echo "  2) 最近20条决策记录"
echo "  3) 统计交易次数"
echo "  4) 统计盈亏"
echo "  5) 查看所有交易（完整）"
echo "  6) 实时监控决策"
echo ""
read -p "请选择 [1-6]: " choice

case $choice in
    1)
        if command -v jq &> /dev/null; then
            echo ""
            echo "最近10条交易："
            tail -10 logs/trades.jsonl | jq -c '{时间: (.timestamp | todate), 动作: .action, 价格: .price, 数量: .amount, 盈亏: .pnl, 理由: .reason}'
        else
            tail -10 logs/trades.jsonl
        fi
        ;;
    2)
        if command -v jq &> /dev/null; then
            echo ""
            echo "最近20条决策："
            tail -20 logs/decisions.jsonl | jq -c '{时间: (.timestamp | todate), 信号: .decision.signal, 信心: .decision.confidence, 理由: .decision.reason}'
        else
            tail -20 logs/decisions.jsonl
        fi
        ;;
    3)
        echo ""
        echo "交易统计："
        echo "  总决策次数: $(wc -l < logs/decisions.jsonl 2>/dev/null || echo 0)"
        echo "  总交易次数: $(wc -l < logs/trades.jsonl 2>/dev/null || echo 0)"
        if command -v jq &> /dev/null; then
            echo "  BUY 信号: $(grep -c '"signal":"BUY"' logs/decisions.jsonl 2>/dev/null || echo 0)"
            echo "  SELL 信号: $(grep -c '"signal":"SELL"' logs/decisions.jsonl 2>/dev/null || echo 0)"
            echo "  HOLD 信号: $(grep -c '"signal":"HOLD"' logs/decisions.jsonl 2>/dev/null || echo 0)"
        fi
        ;;
    4)
        if command -v jq &> /dev/null; then
            echo ""
            echo "盈亏统计："
            cat logs/trades.jsonl | jq -s '[.[] | select(.pnl != null) | .pnl] | {总盈亏: add, 交易次数: length, 平均: (add / length)}'
        else
            echo "需要安装 jq 才能查看盈亏统计"
        fi
        ;;
    5)
        if command -v jq &> /dev/null; then
            cat logs/trades.jsonl | jq
        else
            cat logs/trades.jsonl
        fi
        ;;
    6)
        echo ""
        echo "实时监控决策日志 (Ctrl+C 退出)..."
        echo ""
        if command -v jq &> /dev/null; then
            tail -f logs/decisions.jsonl | jq -c '{时间: (.timestamp | todate), 信号: .decision.signal, 信心: .decision.confidence, 理由: .decision.reason}'
        else
            tail -f logs/decisions.jsonl
        fi
        ;;
    *)
        echo "无效选择"
        exit 1
        ;;
esac
