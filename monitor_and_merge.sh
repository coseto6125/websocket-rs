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
    # gh 2.79 只認 `state`,不認 `conclusion`:問錯欄位整個查詢會失敗,
    # CI_CONCLUSION 因此永遠是空字串,迴圈永遠走不到合併那一步。
    # `|| true` 讓將來欄位再改名時是停下來報錯,不是靜靜地不合併。
    CI_RESULTS=$(gh pr checks "$PR_NUMBER" --json state --jq '.[].state' 2>/dev/null || true)

    echo -e "[${TIMESTAMP}] 檢查 #${ATTEMPT} - CI 狀態: $(echo "$CI_RESULTS" | sort | uniq -c | tr '\n' ' ')"

    # 一次把每個 check 分成三類:已完成且沒問題、還在跑、已完成但不是成功。
    # 只列還在跑的狀態,其餘終局狀態一律當失敗,就不必窮舉 gh 的失敗名稱。
    ALL_SUCCESS=true
    CI_FAILED=false
    if [ -n "$CI_RESULTS" ]; then
        while IFS= read -r state; do
            case "$state" in
                SUCCESS | SKIPPED | NEUTRAL) ;;
                PENDING | QUEUED | IN_PROGRESS | EXPECTED | WAITING | REQUESTED)
                    ALL_SUCCESS=false
                    ;;
                *)
                    ALL_SUCCESS=false
                    CI_FAILED=true
                    break
                    ;;
            esac
        done <<< "$CI_RESULTS"
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
            if ! gh pr merge "$PR_NUMBER" --squash; then
                echo -e "\n${RED}❌ 合併失敗${NC}"
                exit 1
            fi
            echo -e "\n${GREEN}🎉 PR #${PR_NUMBER} 已成功合併!${NC}"

            # 刪分支是善後,不算合併結果的一部分。單一句
            # `gh pr merge --delete-branch` 在本機分支被 worktree 佔住時
            # 會讓整條指令失敗,曾把已經合併的 PR 誤報成合併失敗。
            # 現在兩邊各自回報,刪不掉只警告。
            HEAD_BRANCH=$(gh pr view "$PR_NUMBER" --json headRefName -q .headRefName)
            if gh api --silent -X DELETE "repos/{owner}/{repo}/git/refs/heads/${HEAD_BRANCH}" 2>/dev/null; then
                echo -e "${GREEN}✓ 已刪除遠端分支 ${HEAD_BRANCH}${NC}"
            else
                echo -e "${YELLOW}⚠ 遠端分支 ${HEAD_BRANCH} 未刪除,請手動確認${NC}"
            fi

            # 用 -D 不用 -d:squash merge 之後 git 看這條分支永遠是未合併,
            # 而上面的 PR 狀態已經證明它合併了。
            if git branch -D "$HEAD_BRANCH" 2>/dev/null; then
                echo -e "${GREEN}✓ 已刪除本地分支 ${HEAD_BRANCH}${NC}"
            else
                echo -e "${YELLOW}⚠ 本地分支 ${HEAD_BRANCH} 未刪除(不存在,或被 worktree 佔用)${NC}"
            fi
            echo
            exit 0
        else
            echo -e "${RED}✗ PR 無法合併 (mergeable=${MERGEABLE})${NC}"
            echo -e "${YELLOW}可能有衝突需要解決${NC}"
            exit 1
        fi
    fi

    # 檢查是否有失敗
    if [ "$CI_FAILED" = true ]; then
        echo -e "\n${RED}❌ CI 檢查失敗!${NC}"
        echo -e "${YELLOW}請檢查失敗原因:${NC}"
        gh pr checks "$PR_NUMBER"
        exit 1
    fi

    # 等待下次檢查
    echo -e "${BLUE}等待 ${CHECK_INTERVAL} 秒後再次檢查...${NC}\n"
    sleep $CHECK_INTERVAL
done
