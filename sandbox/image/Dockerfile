FROM debian:trixie-slim
SHELL ["/bin/bash", "-o", "pipefail", "-c"]
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential ca-certificates curl file git jq less nodejs npm openjdk-21-jdk-headless \
        procps python3 python3-pip python3-venv ripgrep sqlite3 unzip xxd xz-utils zip \
    && rm -rf /var/lib/apt/lists/*
COPY --from=ghcr.io/astral-sh/uv:0.12.15 /uv /uvx /usr/local/bin/
ENV UV_TOOL_DIR=/opt/uv-tools UV_TOOL_BIN_DIR=/usr/local/bin
RUN uv tool install apkid==3.1.0
RUN curl -fsSL -o /tmp/jadx.zip https://github.com/skylot/jadx/releases/download/v1.5.6/jadx-1.5.6.zip \
    && unzip -q /tmp/jadx.zip -d /opt/jadx \
    && rm /tmp/jadx.zip \
    && ln -s /opt/jadx/bin/jadx /usr/local/bin/jadx
RUN mkdir -p /opt/android \
    && curl -fsSL -o /opt/android/apktool.jar https://github.com/iBotPeaches/Apktool/releases/download/v3.0.3/apktool_3.0.3.jar \
    && curl -fsSL -o /opt/android/apkeditor.jar https://github.com/REAndroid/APKEditor/releases/download/V1.4.9/APKEditor-1.4.9.jar \
    && curl -fsSL -o /opt/android/bundletool.jar https://github.com/google/bundletool/releases/download/1.18.3/bundletool-all-1.18.3.jar \
    && for tool in apktool apkeditor bundletool; do \
        printf '#!/bin/sh\nexec java -jar /opt/android/%s.jar "$@"\n' "$tool" > "/usr/local/bin/$tool"; \
        chmod +x "/usr/local/bin/$tool"; \
    done
RUN curl -fsSL -o /tmp/temurin17.tar.gz https://github.com/adoptium/temurin17-binaries/releases/download/jdk-17.0.20.1%2B1/OpenJDK17U-jdk_x64_linux_hotspot_17.0.20.1_1.tar.gz \
    && echo "3808d1d15e3ec6bd5b84057fb5d84c33d8a1536a258146bcea2e603fc726e08e  /tmp/temurin17.tar.gz" | sha256sum -c - \
    && mkdir -p /usr/lib/jvm/temurin-17 \
    && tar -xzf /tmp/temurin17.tar.gz -C /usr/lib/jvm/temurin-17 --strip-components=1 \
    && rm /tmp/temurin17.tar.gz
ENV ANDROID_HOME=/opt/android-sdk
RUN curl -fsSL -o /tmp/cmdline-tools.zip https://dl.google.com/android/repository/commandlinetools-linux-16111833_latest.zip \
    && echo "e025545c62a8e64c7559119566a569fb1dec5f60  /tmp/cmdline-tools.zip" | sha1sum -c - \
    && unzip -q /tmp/cmdline-tools.zip -d /tmp/cmdline-tools \
    && mkdir -p "$ANDROID_HOME/cmdline-tools" \
    && mv /tmp/cmdline-tools/cmdline-tools "$ANDROID_HOME/cmdline-tools/latest" \
    && rm -rf /tmp/cmdline-tools /tmp/cmdline-tools.zip \
    && "$ANDROID_HOME/cmdline-tools/latest/bin/sdkmanager" --licenses < <(yes) > /dev/null \
    && "$ANDROID_HOME/cmdline-tools/latest/bin/sdkmanager" "platforms;android-36" "build-tools;36.0.0" > /dev/null
ENV PATH=/opt/android-sdk/cmdline-tools/latest/bin:/opt/android-sdk/build-tools/36.0.0:$PATH
RUN curl -fsSL https://github.com/coursier/coursier/releases/download/v2.1.25/cs-x86_64-pc-linux.gz \
        | gzip -d > /tmp/coursier \
    && chmod +x /tmp/coursier \
    && mkdir -p /opt/smali/lib \
    && /tmp/coursier fetch -r google com.android.tools.smali:smali:3.0.10 com.android.tools.smali:smali-baksmali:3.0.10 \
        | xargs -I{} cp {} /opt/smali/lib/ \
    && rm -rf /tmp/coursier /root/.cache/coursier \
    && printf '#!/bin/sh\nexec java -cp "/opt/smali/lib/*" com.android.tools.smali.smali.Main "$@"\n' > /usr/local/bin/smali \
    && printf '#!/bin/sh\nexec java -cp "/opt/smali/lib/*" com.android.tools.smali.baksmali.Main "$@"\n' > /usr/local/bin/baksmali \
    && chmod +x /usr/local/bin/smali /usr/local/bin/baksmali
RUN git clone --depth 1 https://git.reseam.app/reseam/patches /tmp/patches \
    && version=$(curl -fsS https://git.reseam.app/api/v1/repos/reseam/reseam/releases/latest | jq -er .tag_name) \
    && curl -fsSL -o /tmp/reseam "https://git.reseam.app/reseam/reseam/releases/download/$version/reseam-linux-x64" \
    && chmod +x /tmp/reseam \
    && /tmp/reseam bundle keygen --out /tmp/bundle.key \
    && cd /tmp/patches \
    && RESEAM_BIN=/tmp/reseam RESEAM_BUNDLE_KEY=/tmp/bundle.key ./gradlew bundle --no-daemon --quiet \
    && cd / \
    && rm -rf /tmp/patches /tmp/reseam /tmp/bundle.key* /root/.gradle/daemon
WORKDIR /workspace
