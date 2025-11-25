#!/bin/bash

set -e

PR_NUMBER=3
TAG_NAME="v0.3.1"
BRANCH_NAME="feature/v0.3.1-improvements"

echo "🚀 自動化 PR Merge 和 Release 流程"
echo "=" * 70
echo "PR: #${PR_NUMBER}"
echo "Tag: ${TAG_NAME}"
echo "分支: ${BRANCH_NAME}"
echo "=" * 70
echo

# 步驟 1: 監控 CI
echo "📊 步驟 1: 監控 CI 狀態..."
echo "-" * 70

while true; do
    PR_DATA=$(gh pr view ${PR_NUMBER} --json state,statusCheckRollup,mergeable 2>/dev/null)

    if [ $? -ne 0 ]; then
        echo "❌ 無法取得 PR 狀態"
        exit 1
    fi

    STATE=$(echo "$PR_DATA" | jq -r '.state')

    # 檢查是否已經 merged
    if [ "$STATE" = "MERGED" ]; then
        echo "✅ PR 已經被 merge"
        break
    fi

    # 檢查所有 checks
    PENDING=$(echo "$PR_DATA" | jq '[.statusCheckRollup[] | select(.status == "IN_PROGRESS" or .status == "QUEUED")] | length')
    FAILED=$(echo "$PR_DATA" | jq '[.statusCheckRollup[] | select(.conclusion == "FAILURE" or .conclusion == "CANCELLED")] | length')
    SUCCESS=$(echo "$PR_DATA" | jq '[.statusCheckRollup[] | select(.conclusion == "SUCCESS")] | length')
    TOTAL=$(echo "$PR_DATA" | jq '.statusCheckRollup | length')

    echo -ne "\r⏳ CI 狀態: ${SUCCESS}/${TOTAL} 通過, ${PENDING} 執行中, ${FAILED} 失敗    "

    # 如果有失敗
    if [ "$FAILED" -gt 0 ]; then
        echo ""
        echo "❌ CI 檢查失敗！"
        echo ""
        echo "失敗的檢查："
        echo "$PR_DATA" | jq -r '.statusCheckRollup[] | select(.conclusion == "FAILURE" or .conclusion == "CANCELLED") | "- \(.name): \(.conclusion)"'
        exit 1
    fi

    # 如果全部成功
    if [ "$PENDING" -eq 0 ] && [ "$SUCCESS" -eq "$TOTAL" ] && [ "$TOTAL" -gt 0 ]; then
        echo ""
        echo "✅ 所有 CI 檢查都通過了！"
        echo ""
        break
    fi

    sleep 10
done

# 步驟 2: Squash Merge PR
echo "🔀 步驟 2: Squash Merge PR #${PR_NUMBER}..."
echo "-" * 70

gh pr merge ${PR_NUMBER} --squash --delete-branch --subject "Release v0.3.1: Performance optimizations and API improvements" --body "Squashed merge of PR #${PR_NUMBER}

Performance improvements:
- Error handling: 50x faster (10M+ errors/sec)
- Event loop cache: 25% improvement for non-context-manager usage
- API safety: get_running_loop() for Python 3.10+

Documentation:
- Update installation instructions to prioritize PyPI
- Add comprehensive CHANGELOG.md
- Add Event Loop Caching section to API docs

Code review feedback (Sourcery-AI):
- ✅ Implemented event loop cache write-back (tested, 25% improvement)
- ✅ Responded to all other concerns with design rationale

Code cleanup:
- Remove unused functions and deprecated code
- Fix all clippy warnings and formatting issues

🤖 Generated with [Claude Code](https://claude.com/claude-code)"

if [ $? -ne 0 ]; then
    echo "❌ PR merge 失敗"
    exit 1
fi

echo "✅ PR merge 成功！"
echo

# 等待 GitHub 處理
sleep 3

# 步驟 3: 更新本地 main
echo "🔄 步驟 3: 更新本地 main 分支..."
echo "-" * 70

git checkout main
git pull origin main

echo "✅ Main 分支已更新"
echo

# 步驟 4: 刪除舊的 tag 和 release
echo "🗑️  步驟 4: 刪除舊的 v0.3.1 tag 和 release..."
echo "-" * 70

# 刪除 GitHub release
echo "刪除 GitHub release..."
gh release delete ${TAG_NAME} --yes 2>/dev/null || echo "Release 不存在或已刪除"

# 刪除遠端 tag
echo "刪除遠端 tag..."
git push origin :refs/tags/${TAG_NAME} 2>/dev/null || echo "遠端 tag 不存在或已刪除"

# 刪除本地 tag
echo "刪除本地 tag..."
git tag -d ${TAG_NAME} 2>/dev/null || echo "本地 tag 不存在或已刪除"

echo "✅ 舊的 tag 和 release 已清理"
echo

# 步驟 5: 建立新的 tag
echo "🏷️  步驟 5: 建立新的 v0.3.1 tag..."
echo "-" * 70

git tag -a ${TAG_NAME} -m "Release v0.3.1

Performance optimizations and API improvements

Key improvements:
- Error handling: 50x faster (10M+ errors/sec)
- Event loop cache: 25% improvement for non-context-manager usage
- API safety: get_running_loop() for Python 3.10+

Documentation:
- Comprehensive CHANGELOG.md with performance benchmarks
- Event Loop Caching section in API docs
- Updated README (English and Traditional Chinese)

Code review:
- Addressed Sourcery-AI feedback
- Implemented event loop cache write-back (tested)
- All clippy and formatting checks pass

See CHANGELOG.md for full details."

echo "✅ Tag ${TAG_NAME} 已建立"
echo

# 步驟 6: 推送 tag
echo "📤 步驟 6: 推送 tag 到遠端（觸發 release workflow）..."
echo "-" * 70

git push origin ${TAG_NAME}

if [ $? -eq 0 ]; then
    echo ""
    echo "🎉 完成！"
    echo "=" * 70
    echo "✅ PR merged (squash)"
    echo "✅ Tag ${TAG_NAME} 已建立並推送"
    echo "✅ Release workflow 已觸發"
    echo ""
    echo "📋 後續動作："
    echo "1. Release workflow 將自動："
    echo "   - 在 Ubuntu, Windows, macOS 上建立 wheels"
    echo "   - 建立 GitHub Release"
    echo "   - 上傳到 PyPI"
    echo ""
    echo "2. 監控 workflow:"
    echo "   https://github.com/coseto6125/websocket-rs/actions"
    echo ""
    echo "3. 檢查 release:"
    echo "   https://github.com/coseto6125/websocket-rs/releases/tag/${TAG_NAME}"
    echo ""
else
    echo "❌ Tag 推送失敗"
    exit 1
fi
