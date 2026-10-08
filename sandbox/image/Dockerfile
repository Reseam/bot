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
RUN curl -fsSL -o /tmp/build-tools.zip https://dl.google.com/android/repository/build-tools_r36_linux.zip \
    && unzip -q /tmp/build-tools.zip -d /tmp/build-tools \
    && mv /tmp/build-tools/* /opt/android/build-tools \
    && rm -rf /tmp/build-tools /tmp/build-tools.zip
ENV PATH=/opt/android/build-tools:$PATH
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
WORKDIR /workspace
