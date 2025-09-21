FROM rust:slim

# Install system dependencies for build and runtime
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
      libssl-dev \
      pkg-config \
      build-essential \
      curl \
      git && \
    apt-get clean && rm -rf /var/lib/apt/lists/*

# Set app
WORKDIR /app

# Copy entire project
COPY . .

# Remove any local Cargo override
RUN rm -f .cargo/config.toml

# Accept build args
ARG BUILD_TYPE
ARG PROGRAM

ENV BUILD_TYPE=${BUILD_TYPE}
ENV PROGRAM=${PROGRAM}

RUN echo "🐳  BUILD_TYPE: $BUILD_TYPE"
RUN echo "🐳  PROGRAM: $PROGRAM"

# Build the Rust binary
RUN if [ "$BUILD_TYPE" = "release" ]; then \
      cargo build --release --bin ${PROGRAM}; \
    else \
      cargo build --bin ${PROGRAM}; \
    fi

# Set working directory to project root for config access
WORKDIR /app

# Default command: run built binary based on BUILD_TYPE
CMD [ "bash", "-c", "if [ \"$BUILD_TYPE\" = \"release\" ]; then exec ./target/release/$PROGRAM; else exec ./target/debug/$PROGRAM; fi" ]