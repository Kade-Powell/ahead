#!/bin/sh
set -eux

# This script is written to be as POSIX as possible
# so it works fine for all Unix-like operating systems

test_cmd() {
  command -v "$1" >/dev/null
}

# proxy version
ahead_new_ver="${1}"
# proxy directory
# eval to resolve '~' into proper user dir
eval ahead_dir="'${2}'"

case "${ahead_new_ver}" in
  v*)
    ahead_new_version=$(echo "${ahead_new_ver}" | cut -d'v' -f2)
    ahead_new_ver_tag="${ahead_new_ver}"
  ;;
  nightly*)
    ahead_new_version="${ahead_new_ver}"
    ahead_new_ver_tag=$(echo ${ahead_new_ver} | cut -d '-' -f1)
  ;;
  *)
    printf 'Unknown version\n'
    exit 1
  ;;
esac

if [ -e "${ahead_dir}/ahead" ]; then
  ahead_installed_ver=$("${ahead_dir}/ahead" --version | cut -d' ' -f2)

  printf '[DEBUG]: Current proxy version: %s\n' "${ahead_installed_ver}"
  printf '[DEBUG]: New proxy version: %s\n' "${ahead_new_version}"
  if [ "${ahead_installed_ver}" = "${ahead_new_version}" ]; then
    printf 'Proxy already exists\n'
    exit 0
  else
    printf 'Proxy outdated. Replacing proxy\n'
    rm "${ahead_dir}/ahead"
  fi
fi

for _cmd in tar gzip uname; do
  if ! test_cmd "${_cmd}"; then
    printf 'Missing required command: %s\n' "${_cmd}"
    exit 1
  fi
done

# Currently only linux/darwin are supported
case $(uname -s) in
  Linux) os_name=linux ;;
  Darwin) os_name=darwin ;;
  *)
    printf '[ERROR] unsupported os\n'
    exit 1
  ;;
esac

# Currently only amd64/arm64 are supported
case $(uname -m) in
  x86_64|amd64|x64) arch_name=x86_64 ;;
  arm64|aarch64) arch_name=aarch64 ;;
  # riscv64) arch_name=riscv64 ;;
  *)
    printf '[ERROR] unsupported arch\n'
    exit 1
  ;;
esac

ahead_download_url="https://github.com/Kade-Powell/ahead/releases/download/${ahead_new_ver_tag}/ahead-proxy-${os_name}-${arch_name}.gz"

printf 'Creating "%s"\n' "${ahead_dir}"
mkdir -p "${ahead_dir}"
cd "${ahead_dir}"

if test_cmd 'curl'; then
  # How old curl has these options? we'll find out
  printf 'Downloading using curl\n'
  curl --proto '=https' --tlsv1.2 -LfS -O "${ahead_download_url}"
  # curl --proto '=https' --tlsv1.2 -LZfS -o "${tmp_dir}/ahead-proxy-${os_name}-${arch_name}.gz" "${ahead_download_url}"
elif test_cmd 'wget'; then
  printf 'Downloading using wget\n'
  wget "${ahead_download_url}"
else
  printf 'curl/wget not found, failed to download proxy\n'
  exit 1
fi

printf 'Decompressing gzip\n'
gzip -df "${ahead_dir}/ahead-proxy-${os_name}-${arch_name}.gz"

printf 'Renaming proxy \n'
mv -v "${ahead_dir}/ahead-proxy-${os_name}-${arch_name}" "${ahead_dir}/ahead"

printf 'Making it executable\n'
chmod +x "${ahead_dir}/ahead"

printf 'ahead-proxy installed\n'

exit 0
