#!/bin/sh
# Vercel's Ignored Build Step, run from site/: exit 0 skips the deployment, any other code builds it.
# The website is site/ plus the app version it reads from ../package.json and its dependencies,
# locked in ../package-lock.json, so a push that touches none of them, such as work on the app,
# does not deploy it.

# The last successful deployment of this branch. A new preview branch has none, so take the commit
# of main it branched from, fetched from GitHub because Vercel clones only the branch. When there is
# no such commit or it is not in the shallow history, git fails and the site builds.
base=$VERCEL_GIT_PREVIOUS_SHA
if [ -z "$base" ] && [ "$VERCEL_ENV" = preview ]; then
  git fetch -q --depth=50 https://github.com/tomasmach/nuzky.git main && base=$(git merge-base HEAD FETCH_HEAD)
fi
[ -n "$base" ] || exit 1
echo "Changes to the website since $base:"
git diff --exit-code --stat "$base" HEAD -- . ../package.json ../package-lock.json
