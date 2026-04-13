#!/bin/bash
# 監控 PR 狀態並自動合併
# 用途: 監控指定 PR 的 CI 狀態,通過後自動合併

set -e

# 顏色定義
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

# 檢查參數
if [ -z "$1" ]; then
    echo -e "${RED}錯誤: 需要提供 PR 編號${NC}"
    echo "用法: ./monitor_and_merge.sh <PR編號>"
    echo "範例: ./monitor_and_merge.sh 5"
    exit 1
fi

PR_NUMBER=$1
CHECK_INTERVAL=30  # 檢查間隔(秒)

echo -e "${BLUE}======================================${NC}"
echo -e "${BLUE}  PR #${PR_NUMBER} 自動合併監控器${NC}"
echo -e "${BLUE}======================================${NC}\n"

# 檢查 gh CLI
if ! command -v gh &> /dev/null; then
    echo -e "${RED}錯誤: 需要安裝 GitHub CLI (gh)${NC}"
    echo "安裝: https://cli.github.com/"
    exit 1
fi

# 檢查 PR 是否存在
if ! gh pr view "$PR_NUMBER" &> /dev/null; then
    echo -e "${RED}錯誤: PR #${PR_NUMBER} 不存在${NC}"
    exit 1
fi

echo -e "${GREEN}✓ PR #${PR_NUMBER} 找到${NC}"
echo -e "${YELLOW}開始監控 CI 狀態...${NC}\n"

# 主監控循環
ATTEMPT=0
while true; do
    ATTEMPT=$((ATTEMPT + 1))
    TIMESTAMP=$(date '+%Y-%m-%d %H:%M:%S')

    # 取得 PR 狀態
    PR_STATE=$(gh pr view "$PR_NUMBER" --json state -q .state)

    if [ "$PR_STATE" != "OPEN" ]; then
        echo -e "\n${YELLOW}[${TIMESTAMP}] PR 已經不是 OPEN 狀態: ${PR_STATE}${NC}"
        exit 0
    fi

    # 檢查 CI 狀態
    CI_STATUS=$(gh pr checks "$PR_NUMBER" --json state,conclusion -q '.[].state' 2>/dev/null || echo "PENDING")
    CI_CONCLUSION=$(gh pr checks "$PR_NUMBER" --json state,conclusion -q '.[].conclusion' 2>/dev/null || echo "")

    echo -e "[${TIMESTAMP}] 檢查 #${ATTEMPT} - CI 狀態: ${CI_STATUS}"

    # 檢查是否全部成功
    ALL_SUCCESS=true
    if [ -n "$CI_CONCLUSION" ]; then
        while IFS= read -r conclusion; do
            if [ "$conclusion" != "SUCCESS" ] && [ "$conclusion" != "SKIPPED" ]; then
                ALL_SUCCESS=false
                break
            fi
        done <<< "$(gh pr checks "$PR_NUMBER" --json conclusion -q '.[].conclusion')"
    else
        ALL_SUCCESS=false
    fi

    if [ "$ALL_SUCCESS" = true ]; then
        echo -e "\n${GREEN}✅ 所有 CI 檢查通過!${NC}"

        # 檢查是否可合併
        MERGEABLE=$(gh pr view "$PR_NUMBER" --json mergeable -q .mergeable)

        if [ "$MERGEABLE" = "MERGEABLE" ]; then
            echo -e "${GREEN}✓ PR 可以合併${NC}"
            echo -e "${YELLOW}正在合併...${NC}"

            # 執行合併
            if gh pr merge "$PR_NUMBER" --squash --delete-branch; then
                echo -e "\n${GREEN}🎉 PR #${PR_NUMBER} 已成功合併!${NC}\n"
                exit 0
            else
                echo -e "\n${RED}❌ 合併失敗${NC}"
                exit 1
            fi
        else
            echo -e "${RED}✗ PR 無法合併 (mergeable=${MERGEABLE})${NC}"
            echo -e "${YELLOW}可能有衝突需要解決${NC}"
            exit 1
        fi
    fi

    # 檢查是否有失敗
    if echo "$CI_CONCLUSION" | grep -q "FAILURE"; then
        echo -e "\n${RED}❌ CI 檢查失敗!${NC}"
        echo -e "${YELLOW}請檢查失敗原因:${NC}"
        gh pr checks "$PR_NUMBER"
        exit 1
    fi

    # 等待下次檢查
    echo -e "${BLUE}等待 ${CHECK_INTERVAL} 秒後再次檢查...${NC}\n"
    sleep $CHECK_INTERVAL
done
