#!/bin/bash

# 監控 CI 並自動 merge 和 tag
PR_NUMBER=3
TAG_NAME="v0.3.1"
BRANCH_NAME="feature/v0.3.1-improvements"

echo "🔍 開始監控 PR #${PR_NUMBER} 的 CI 狀態..."
echo "================================"

while true; do
    # 取得 PR 狀態
    PR_DATA=$(gh pr view ${PR_NUMBER} --json state,statusCheckRollup,mergeable 2>/dev/null)

    if [ $? -ne 0 ]; then
        echo "❌ 無法取得 PR 狀態"
        exit 1
    fi

    STATE=$(echo "$PR_DATA" | jq -r '.state')
    MERGEABLE=$(echo "$PR_DATA" | jq -r '.mergeable')

    # 檢查是否已經 merged
    if [ "$STATE" = "MERGED" ]; then
        echo "✅ PR 已經被 merge"
        break
    fi

    # 檢查所有 checks 的狀態
    PENDING=$(echo "$PR_DATA" | jq '[.statusCheckRollup[] | select(.status == "IN_PROGRESS" or .status == "QUEUED")] | length')
    FAILED=$(echo "$PR_DATA" | jq '[.statusCheckRollup[] | select(.conclusion == "FAILURE" or .conclusion == "CANCELLED")] | length')
    SUCCESS=$(echo "$PR_DATA" | jq '[.statusCheckRollup[] | select(.conclusion == "SUCCESS")] | length')
    TOTAL=$(echo "$PR_DATA" | jq '.statusCheckRollup | length')

    # 顯示目前狀態
    echo -ne "\r⏳ CI 狀態: ${SUCCESS}/${TOTAL} 通過, ${PENDING} 執行中, ${FAILED} 失敗    "

    # 如果有失敗的 checks
    if [ "$FAILED" -gt 0 ]; then
        echo ""
        echo "❌ CI 檢查失敗！"
        echo ""
        echo "失敗的檢查："
        echo "$PR_DATA" | jq -r '.statusCheckRollup[] | select(.conclusion == "FAILURE" or .conclusion == "CANCELLED") | "- \(.name): \(.conclusion)"'
        exit 1
    fi

    # 如果所有 checks 都成功
    if [ "$PENDING" -eq 0 ] && [ "$SUCCESS" -eq "$TOTAL" ] && [ "$TOTAL" -gt 0 ]; then
        echo ""
        echo "✅ 所有 CI 檢查都通過了！"
        echo ""

        # Merge PR
        echo "🔀 正在 merge PR #${PR_NUMBER}..."
        gh pr merge ${PR_NUMBER} --squash --delete-branch --subject "Release v0.3.1: Performance optimizations and API improvements" --body "Squashed merge of PR #${PR_NUMBER}

Performance improvements:
- Error handling: 50x faster (10M+ errors/sec)
- Event loop cache: Per-connection optimization
- API safety: get_running_loop() for Python 3.10+

Documentation:
- Update installation instructions to prioritize PyPI
- Add comprehensive CHANGELOG.md

Code cleanup:
- Remove unused functions and deprecated code
- Fix all clippy warnings and formatting issues

🤖 Generated with [Claude Code](https://claude.com/claude-code)"

        if [ $? -eq 0 ]; then
            echo "✅ PR merge 成功！"
            echo ""

            # 等待一下讓 GitHub 處理 merge
            sleep 3

            # 切換到 main 並拉取最新版本
            echo "🔄 更新本地 main 分支..."
            git checkout main
            git pull origin main

            # 檢查 tag 是否已存在
            if git rev-parse "${TAG_NAME}" >/dev/null 2>&1; then
                echo "⚠️  Tag ${TAG_NAME} 已存在，刪除舊的 tag..."
                git tag -d ${TAG_NAME}
                git push origin :refs/tags/${TAG_NAME}
            fi

            # 建立新的 tag
            echo "🏷️  建立 tag ${TAG_NAME}..."
            git tag -a ${TAG_NAME} -m "Release v0.3.1

Performance optimizations and API improvements

Key improvements:
- Error handling: 50x faster (10M+ errors/sec)
- Event loop cache: Per-connection optimization
- API safety: get_running_loop() for Python 3.10+

See CHANGELOG.md for full details."

            # 推送 tag
            echo "📤 推送 tag 到遠端..."
            git push origin ${TAG_NAME}

            if [ $? -eq 0 ]; then
                echo ""
                echo "🎉 完成！"
                echo "================================"
                echo "✅ PR merged"
                echo "✅ Tag ${TAG_NAME} 已建立並推送"
                echo ""
                echo "查看 release: https://github.com/coseto6125/websocket-rs/releases/tag/${TAG_NAME}"
            else
                echo "❌ Tag 推送失敗"
                exit 1
            fi
        else
            echo "❌ PR merge 失敗"
            exit 1
        fi

        break
    fi

    # 等待 10 秒後再檢查
    sleep 10
done
