#!/bin/sh
# Vercel's Ignored Build Step, run from site/: exit 0 skips the deployment, any other code builds it.
# The website is site/ plus the app version it reads from ../package.json and its dependencies,
# locked in ../package-lock.json, so a push that touches none of them, such as work on the app,
# does not deploy it.

# The last successful deployment of this branch. A new branch has none, so take the main commit it
# started from: the newest merge of a pull request, since main merges them with merge commits and
# merging main into a branch does not say "pull request". When neither is in Vercel's shallow
# clone, git fails and the site builds.
base=${VERCEL_GIT_PREVIOUS_SHA:-$(git rev-list --first-parent -n 1 --grep='^Merge pull request #' HEAD^)}
[ -n "$base" ] || exit 1
echo "Changes to the website since $base:"
git diff --exit-code --stat "$base" HEAD -- . ../package.json ../package-lock.json
