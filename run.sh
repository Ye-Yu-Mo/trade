#!/bin/bash

# 多智能体加密货币自动交易系统 - 启动脚本

set -e

echo "============================================================"
echo "  多智能体加密货币自动交易系统"
echo "============================================================"
echo ""

# 检查 .env 文件
if [ ! -f .env ]; then
    echo "❌ 错误: .env 文件不存在"
    echo "请执行: cp .env.example .env"
    echo "然后编辑 .env 文件，填入你的 API 密钥"
    exit 1
fi

# 检查 API 密钥是否仍是占位符
if grep -qE "your_testnet_api_key|your_testnet_secret|your_deepseek_key" .env; then
    echo "⚠️  警告: 检测到未配置的 API 密钥占位符"
    echo ""
    echo "请编辑 .env 文件，替换以下占位符："
    echo "  - BINANCE_API_KEY=your_testnet_api_key"
    echo "  - BINANCE_SECRET=your_testnet_secret"
    echo "  - DEEPSEEK_API_KEY=your_deepseek_key"
    echo ""
    read -p "是否继续运行？(可能会失败) [y/N]: " confirm
    if [[ ! $confirm =~ ^[Yy]$ ]]; then
        exit 0
    fi
fi

echo "✓ 环境配置检查通过"
echo ""

# 创建日志目录
mkdir -p logs

# 编译项目
echo "正在编译项目..."
cargo build --release

echo ""
echo "============================================================"
echo "  启动交易机器人"
echo "============================================================"
echo ""
echo "按 Ctrl+C 停止程序"
echo ""

# 运行程序
cargo run --release
