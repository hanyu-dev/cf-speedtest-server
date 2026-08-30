# tespeed

## Building

### Binary

```bash
cargo build --locked --release --package tespeed-bin
```

To deploy to Cloudflare Workers, you can use the following command:

```bash
cd crates/tespeed-worker && npx wrangler deploy
```

### OCI image

To check for reproducibility, you can build the image locally using the following commands:

```bash
export IMAGE_BUILDAH_VERSION=1.43.2-immutable
export IMAGE_BUILDAH_DIGEST=19ee6da290f77e63ab7f108a1b22e63d4c36cb827fba9f494982c5ceeba27019

export IMAGE_REGISTRY=ghcr.io
export IMAGE_NAME=tespeed
export IMAGE_BUILD_PLATFORMS=linux/amd64,linux/arm64

podman build \
    --build-arg IMAGE_BUILDAH_VERSION="${IMAGE_BUILDAH_VERSION}" \
    --build-arg IMAGE_BUILDAH_DIGEST="${IMAGE_BUILDAH_DIGEST}" \
    -f Dockerfile.buildah \
    -t localhost/buildah/stable:tespeed

podman run \
    --rm \
    --device /dev/fuse \
    --security-opt label=disable \
    -v "$PWD:/workspace" \
    -w /workspace \
    -e IMAGE_REGISTRY="${IMAGE_REGISTRY}" \
    -e IMAGE_NAME="${IMAGE_NAME}" \
    -e IMAGE_BUILD_PLATFORMS="${IMAGE_BUILD_PLATFORMS}" \
    --user=root \
    localhost/buildah/stable:tespeed \
    ./build.sh
```

## License

MIT OR Apache-2.0
