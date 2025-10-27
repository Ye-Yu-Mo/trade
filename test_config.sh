#!/bin/bash

# 配置测试脚本 - 验证 API 连接

echo "============================================================"
echo "  配置测试 - 验证 API 连接"
echo "============================================================"
echo ""

# 检查 .env 文件
if [ ! -f .env ]; then
    echo "❌ .env 文件不存在"
    exit 1
fi

source .env

echo "正在测试配置..."
echo ""

# 1. 测试 Binance API
echo "1. 测试 Binance 测试网连接..."

# 根据环境变量选择 URL
if [ "$BINANCE_TESTNET" = "true" ]; then
    BINANCE_URL="https://testnet.binancefuture.com"
    echo "   使用测试网: $BINANCE_URL"
else
    BINANCE_URL="https://fapi.binance.com"
    echo "   使用主网: $BINANCE_URL"
fi

BINANCE_RESPONSE=$(curl -s "$BINANCE_URL/fapi/v1/ticker/price?symbol=BTCUSDT")

if echo "$BINANCE_RESPONSE" | grep -q "price"; then
    PRICE=$(echo "$BINANCE_RESPONSE" | grep -o '"price":"[^"]*"' | cut -d'"' -f4)
    echo "   ✓ Binance API 连接成功"
    echo "   ✓ BTCUSDT 当前价格: \$$PRICE"
else
    echo "   ❌ Binance API 连接失败"
    echo "   响应: $BINANCE_RESPONSE"
fi

echo ""

# 2. 测试 DeepSeek API
echo "2. 测试 DeepSeek API 连接..."
DEEPSEEK_RESPONSE=$(curl -s https://api.deepseek.com/v1/models \
  -H "Authorization: Bearer $DEEPSEEK_API_KEY" \
  -H "Content-Type: application/json")

if echo "$DEEPSEEK_RESPONSE" | grep -q "deepseek-chat"; then
    echo "   ✓ DeepSeek API 连接成功"
    echo "   ✓ API Key 有效"
else
    echo "   ❌ DeepSeek API 连接失败"
    if echo "$DEEPSEEK_RESPONSE" | grep -q "invalid"; then
        echo "   ❌ API Key 无效，请检查 DEEPSEEK_API_KEY"
    else
        echo "   响应: $DEEPSEEK_RESPONSE"
    fi
fi

echo ""

# 3. 显示交易配置
echo "3. 当前交易配置:"
echo "   交易对: $TRADE_SYMBOL"
echo "   交易数量: $TRADE_AMOUNT"
echo "   交易周期: $TRADE_INTERVAL"

echo ""
echo "============================================================"
echo "  测试完成"
echo "============================================================"
echo ""

# 提示下一步
if echo "$BINANCE_RESPONSE" | grep -q "price" && echo "$DEEPSEEK_RESPONSE" | grep -q "deepseek-chat"; then
    echo "✓ 所有配置正确，可以运行程序："
    echo "  ./run.sh"
else
    echo "⚠️  部分配置有问题，请检查 .env 文件"
fi
