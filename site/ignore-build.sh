#!/bin/sh
# Vercel's Ignored Build Step, run from site/: exit 0 skips the deployment, exit 1 builds it, and any
# other code fails it, so every git error below must end in exit 1.
# The website is site/ plus the app version it reads from ../package.json and its dependencies,
# locked in ../package-lock.json, so a push that touches none of them, such as work on the app,
# does not deploy it.

repo=https://github.com/tomasmach/nuzky.git

# The last successful deployment of this branch. A new preview branch has none, so take the commit
# of main it branched from, fetched from GitHub because Vercel clones only the branch.
base=$VERCEL_GIT_PREVIOUS_SHA
if [ -z "$base" ] && [ "$VERCEL_ENV" = preview ]; then
  git fetch -q --depth=50 "$repo" main && base=$(git merge-base HEAD FETCH_HEAD)
fi
[ -n "$base" ] || exit 1
# Vercel clones only the last few commits, and skipped pushes do not move the base, so after a run
# of app-only merges it falls out of the clone.
git cat-file -e "$base^{commit}" 2>/dev/null || git fetch -q --depth=1 "$repo" "$base"
echo "Changes to the website since $base:"
git diff --exit-code --stat "$base" HEAD -- . ../package.json ../package-lock.json || exit 1
