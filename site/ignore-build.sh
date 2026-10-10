#!/bin/sh
# Vercel's Ignored Build Step, run from site/: exit 0 skips the deployment, any other code builds it.
# The website is site/ plus the app version it reads from ../package.json, so a push that touches
# neither, such as work on the app, does not deploy it.

# The last successful deployment of this branch. A new branch has none, so take the main commit it
# started from: main merges pull requests with merge commits. When neither is in Vercel's shallow
# clone, git fails and the site builds.
base=${VERCEL_GIT_PREVIOUS_SHA:-$(git rev-list --first-parent --merges -n 1 HEAD^)}
[ -n "$base" ] || exit 1
echo "Changes to the website since $base:"
git diff --exit-code --stat "$base" HEAD -- . ../package.json
