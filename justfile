reseam_version := "0.28.0"
sandbox := "root/usr/local/lib/reseam-bot/sandbox"

check:
    cargo build --locked
    npm --prefix sandbox ci
    npm --prefix sandbox run build

release:
    cargo build --release --locked
    npm --prefix sandbox ci
    npm --prefix sandbox run build
    npm --prefix sandbox prune --omit=dev --omit=optional
    rm -rf root dist
    mkdir -p {{ sandbox }} root/usr/local/bin root/etc/reseam-bot dist
    cp -r sandbox/dist sandbox/container sandbox/package.json sandbox/node_modules {{ sandbox }}/
    cp target/release/reseam-bot root/usr/local/bin/
    cp config.toml root/etc/reseam-bot/
    tar --owner=0 --group=0 --numeric-owner -C root -czf dist/reseam-bot-linux-x64.tar.gz .
    curl -fsSL --create-dirs -o target/reseam https://cdn.reseam.app/engine/v{{ reseam_version }}/reseam-linux-x64
    chmod +x target/reseam
    git tag -f latest
    git push -f origin latest
    git log -1 --format='- %s' > target/notes.md
    target/reseam release files dist --tag latest --notes target/notes.md
    curl -fsS -X POST -H "Content-Type: application/json" -H "X-Gitea-Event: push" \
        -d '{"ref": "refs/heads/main", "after": "'"$GITHUB_SHA"'"}' "$DOKPLOY_WEBHOOK"
