FROM kraken/tentacle-base
USER root
ARG DEBIAN_FRONTEND=noninteractive
# build-essential: the tree-sitter grammar's C build (crates/al-syntax/build.rs); cmake: libz-ng-sys
# (CI's runner has it preinstalled); python3: the bench and trace scripts; python-is-python3: the
# commands and hooks call `python` (agentflow, run-timer); libicu: .NET aborts without it
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential cmake pkg-config python3 python-is-python3 jq libicu-dev ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
# Rust as rust-toolchain.toml pins it, outside the cache volume (the base points CARGO_HOME at ~/.cache
# for the registry). A toolchain bump in the repo makes rustup install the new one on first use.
ENV RUSTUP_HOME=/opt/rustup
RUN curl -fsSL https://sh.rustup.rs | CARGO_HOME=/opt/cargo sh -s -- -y --profile minimal \
      --default-toolchain 1.96.0 --component rustfmt,clippy,rust-analyzer --no-modify-path
ENV PATH=/opt/cargo/bin:$PATH
# cargo-nextest: ci-steps test, check-goldens and the pre-commit hook refuse to run without it (prebuilt binary)
RUN curl -fsSL https://get.nexte.st/latest/linux | tar -xzf - -C /opt/cargo/bin && cargo-nextest nextest --version
# .NET 10 SDK and the AL dev tools (`al`, incl. `al graph`): the same tool package as on Windows. The
# ...Development.Tools.Linux package holds only the analyzers, not the CLI (spike 2026-10-19).
RUN curl -fsSL https://dot.net/v1/dotnet-install.sh -o /tmp/dotnet-install.sh \
    && bash /tmp/dotnet-install.sh --channel 10.0 --install-dir /opt/dotnet && rm /tmp/dotnet-install.sh
ENV DOTNET_ROOT=/opt/dotnet PATH=/opt/dotnet:/opt/al-tool:$PATH DOTNET_CLI_TELEMETRY_OPTOUT=1
ARG AL_TOOL_VERSION=18.0.41.62505
# a throwaway HOME, so dotnet and NuGet leave no root-owned ~/.dotnet or ~/.nuget in /home/dev
RUN mkdir -p /tmp/build-home && export HOME=/tmp/build-home \
    && dotnet tool install microsoft.dynamics.businesscentral.development.tools \
      --version $AL_TOOL_VERSION --tool-path /opt/al-tool \
    && al --version && rm -rf /tmp/build-home
# Node for the pi coding agent and pi-mcp (/orchestrate's reviewer models), as in LethAL's image.
# pi's login and state go on the config volume (PI_CODING_AGENT_DIR) so a recreation keeps them.
RUN curl -fsSL https://deb.nodesource.com/setup_lts.x | bash - && apt-get install -y nodejs \
    && rm -rf /var/lib/apt/lists/*
RUN HOME=/tmp/build-home npm install -g --cache /tmp/npm-cache @earendil-works/pi-coding-agent \
    && HOME=/tmp/build-home pi --version && rm -rf /tmp/npm-cache /tmp/build-home
ENV PI_CODING_AGENT_DIR=/home/dev/.config/pi
# Login shells (bash -l: kraken's setup, ssh sessions) rebuild PATH from /etc/profile, so ENV alone is not enough
RUN printf '%s\n' 'export PATH=/opt/cargo/bin:/opt/dotnet:/opt/al-tool:$PATH' 'export DOTNET_ROOT=/opt/dotnet' \
      'export RUSTUP_HOME=/opt/rustup' 'export DOTNET_CLI_TELEMETRY_OPTOUT=1' \
      'export PI_CODING_AGENT_DIR=/home/dev/.config/pi' > /etc/profile.d/al-sem-tools.sh \
    && chown -R dev:dev /opt/rustup /opt/cargo
# ends as root: the base entrypoint drops to dev
