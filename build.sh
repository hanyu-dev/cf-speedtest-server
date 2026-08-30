#!/bin/bash

set -euo pipefail

# ================================
# Variables

PUSH=false

IMAGE_REGISTRY="${IMAGE_REGISTRY:-}"
IMAGE_NAME="${IMAGE_NAME:-}"
IMAGE_BUILD_PLATFORMS="${IMAGE_BUILD_PLATFORMS:-linux/amd64}"

while [[ $# -gt 0 ]]; do
	case "$1" in
	--push)
		PUSH=true
		shift
		;;
	--image-registry)
		IMAGE_REGISTRY="$2"
		shift 2
		;;
	--image-name)
		IMAGE_NAME="$2"
		shift 2
		;;
	--platforms)
		IMAGE_BUILD_PLATFORMS="$2"
		shift 2
		;;
	*)
		echo "Unknown option: '$1'"
		exit 1
		;;
	esac
done

if [ -z "$IMAGE_REGISTRY" ] || [ -z "$IMAGE_NAME" ] || [ -z "$IMAGE_BUILD_PLATFORMS" ]; then
	echo "Missing required variables!"
	exit 1
fi

IMAGE_REPO="${IMAGE_REGISTRY}/${IMAGE_NAME}"

IMAGE_TESPEED_VERSION=$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.name == "tespeed-bin") | .version | split("+")[0]')

IMAGE_TESPEED_VERSION_MAJOR=$(echo "${IMAGE_TESPEED_VERSION%%-*}" | awk -F. '{print $1}')
IMAGE_TESPEED_VERSION_MINOR=$(echo "${IMAGE_TESPEED_VERSION%%-*}" | awk -F. '{print $2}')
IMAGE_TESPEED_VERSION_PATCH=$(echo "${IMAGE_TESPEED_VERSION%%-*}" | awk -F. '{print $3}')

IMAGE_TESPEED_VERSION_MAJOR=${IMAGE_TESPEED_VERSION_MAJOR:-0}
IMAGE_TESPEED_VERSION_MINOR=${IMAGE_TESPEED_VERSION_MINOR:-0}
IMAGE_TESPEED_VERSION_PATCH=${IMAGE_TESPEED_VERSION_PATCH:-0}

IMAGE_TESPEED_VERSION_IS_PRE_RELEASE=false
if [[ "$IMAGE_TESPEED_VERSION" == *"-"* ]]; then
	IMAGE_TESPEED_VERSION_IS_PRE_RELEASE=true
fi

IMAGE_TAG_ROLLING="${IMAGE_REPO}:rolling"
IMAGE_TAG_DEV_FULL_QUALIFIED="${IMAGE_REPO}:${IMAGE_TESPEED_VERSION}"
IMAGE_TAG_RELEASE_MAJOR_MINOR_PATCH="${IMAGE_REPO}:${IMAGE_TESPEED_VERSION_MAJOR}.${IMAGE_TESPEED_VERSION_MINOR}.${IMAGE_TESPEED_VERSION_PATCH}"
IMAGE_TAG_RELEASE_MAJOR_MINOR="${IMAGE_REPO}:${IMAGE_TESPEED_VERSION_MAJOR}.${IMAGE_TESPEED_VERSION_MINOR}"
IMAGE_TAG_RELEASE_MAJOR="${IMAGE_REPO}:${IMAGE_TESPEED_VERSION_MAJOR}"
IMAGE_TAG_RELEASE_LATEST="${IMAGE_REPO}:latest"

# ================================
# VCS Information

git config --global --add safe.directory "*" 2>/dev/null || true

IMAGE_VCS_DATE_EPOCH=$(git log -1 --pretty=%ct)
IMAGE_VCS_DATE=$(date -u -d @$IMAGE_VCS_DATE_EPOCH +'%Y-%m-%dT%H:%M:%S+00:00')
IMAGE_VCS_REV=$(git rev-parse HEAD)

# ================================
# Build

LOCAL_MANIFEST="localhost/${IMAGE_NAME}:build"

buildah manifest rm "${LOCAL_MANIFEST}" 2>/dev/null || true

buildah build \
	--no-cache \
	--platform="${IMAGE_BUILD_PLATFORMS}" \
	--manifest="${LOCAL_MANIFEST}" \
	--jobs=4 \
	--timestamp=${IMAGE_VCS_DATE_EPOCH} \
	--build-arg IMAGE_VCS_DATE=$IMAGE_VCS_DATE \
	--build-arg IMAGE_VCS_REV=$IMAGE_VCS_REV \
	--build-arg IMAGE_TESPEED_VERSION=$IMAGE_TESPEED_VERSION \
	-f Dockerfile .

# ================================
# Push

if [ "$PUSH" = true ]; then
	echo "$IMAGE_REGISTRY_PASSWORD" | buildah login "$IMAGE_REGISTRY" -u "$IMAGE_REGISTRY_USERNAME" --password-stdin

	if [ "$IMAGE_TESPEED_VERSION_IS_PRE_RELEASE" = true ]; then
		echo "Pre-release version detected, skipping release tags..."

		TAGS=(
			"${IMAGE_TAG_ROLLING}"
			"${IMAGE_TAG_DEV_FULL_QUALIFIED}"
		)
	else
		TAGS=(
			"${IMAGE_TAG_ROLLING}"
			"${IMAGE_TAG_RELEASE_MAJOR_MINOR_PATCH}"
			"${IMAGE_TAG_RELEASE_MAJOR_MINOR}"
			"${IMAGE_TAG_RELEASE_MAJOR}"
			"${IMAGE_TAG_RELEASE_LATEST}"
		)
	fi

	for TAG in "${TAGS[@]}"; do
		echo "Pushing images, tagging: ${TAG}..."
		buildah manifest push --all "${LOCAL_MANIFEST}" "docker://${TAG}"
	done
else
	echo "Skipping images push..."
fi
