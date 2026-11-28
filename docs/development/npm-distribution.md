# npm trusted publishing 게시와 provenance 운영

게시 인증은 [ADR-0095](../adr/0095-npm-trusted-publishing.md)를 따른다. granular token 방식
([ADR-0034](../adr/0034-npm-token-publishing-policy.md))은 rc.10까지 썼고 대체됐다.

`@xgen-ai/cli`는 native XGEN를 npm으로 설치하기 위한 선택형 배포 계층이다. 직접 GitHub installer를 쓰는
사용자는 Node.js가 필요 없고, npm 경로를 고른 사용자는 Node.js 22.14 이상이 필요하다. 두 경로는 같은
release binary를 실행한다.

이 package 이름은 설치 entrypoint를 뜻하며 화면 구현을 뜻하지 않는다. 현재 가벼운 REPL이나 향후 같은
`xgen` binary에 포함될 TUI는 동일한 `@xgen-ai/cli`로 배포할 수 있다. TUI를 별도 제품·release cadence로
분리할 때만 별도 package naming ADR을 먼저 작성한다.

## Package 구성

| Package | 대상 |
| --- | --- |
| `@xgen-ai/cli` | OS/CPU 선택과 native process 실행만 담당하는 launcher |
| `@xgen-ai/cli-linux-x64-musl` | Linux x86-64 |
| `@xgen-ai/cli-linux-arm64-musl` | Linux ARM64 |
| `@xgen-ai/cli-darwin-x64` | macOS Intel |
| `@xgen-ai/cli-darwin-arm64` | macOS Apple Silicon |
| `@xgen-ai/cli-win32-x64` | Windows x86-64 |

모든 package는 같은 exact version으로 게시한다. Lifecycle script, 설치 중 GitHub download와 native
compile은 없다. Launcher는 현재 platform package가 없으면 종료한다. `--omit=optional` 설치는 지원하지
않는다.

## 최초 1회 운영 준비

이 절차는 release tag를 만들기 전에 npm scope 관리자 한 명이 수행한다.

1. 해당 계정 또는 npm organization이 `@xgen-ai` scope 여섯 package를 public으로 게시할 권한이 있는지
   확인한다. 권한이나 scope 확보 여부가 불명확하면 `@xgen-ai/cli`를 게시하지 말고 naming ADR을 먼저
   갱신한다.
2. (이미 존재하는 package는 건너뛴다) 최초 bootstrap 게시에만 2FA가 켜진 계정의 일회성 로그인이나
   짧은 수명 token을 현재 process에만 쓴다. Token 값은 repository, release artifact, 문서와 명령 출력에
   넣지 않는다.
3. clean `main` checkout에서 bootstrap tarball을 만든다.

```bash
npm ci --ignore-scripts --prefix npm
bootstrap_dir=$(mktemp -d "${TMPDIR:-/tmp}/xgen-npm-bootstrap.XXXXXX")
node npm/scripts/bootstrap.mjs --output-dir "$bootstrap_dir"
```

4. `npm-bootstrap-manifest.json`의 package name, version, integrity를 확인하고 각 tarball에
   `package.json`, `README.md`, `LICENSE`만 있는지
   `npm publish --dry-run --ignore-scripts --tag bootstrap --json TARBALL`로 검토한다.
5. 동일 token을 현재 process에만 secret-injection하고 여섯 tarball을 platform 우선, launcher 마지막
   순서로 `--access public --tag bootstrap` 게시한다. Token 값을 shell script나 npmrc에 literal로 쓰지
   않는다. Bootstrap에는 실행 파일과 `bin` entry가 없고 GitHub provenance가 없는 package-name 예약용
   version이다.
6. 여섯 package에 trusted publisher를 등록한다(2FA 필요, npm 11.15 이상).
   `npm trust github "@xgen-ai/<package>" --file release.yml --repository PlateerLab/xgen-cli --allow-publish --yes`
   를 package마다 실행하고, npmjs.com에서 launcher `@xgen-ai/cli` 설정에만 `Allow npm dist-tag`를 켠다.
   새 설정은 2일 안에 첫 게시가 없으면 만료되므로 release tag 직전에 한다. 그 뒤 repository variable
   `XGEN_NPM_PUBLISH_ENABLED=true`를 둔다. 이 값은 등록을 사람이 확인했다는 fail-closed acknowledgement다.
   첫 OIDC 게시가 검증되면 package 설정을 "Require two-factor authentication and disallow tokens"로 바꾸고
   이전 token과 repository secret을 폐기한다.
7. RC3 게시와 검증이 끝나면 bootstrap version을 deprecated 처리한다. Package 자체는 unpublish하지
   않는다.

## Release 동작

보호 tag가 현재 `main`과 일치하면 기존 `release.yml`이 다음 순서로 실행한다.

```text
quality + 5-platform native build
  -> native/npm pack + install smoke
  -> immutable GitHub Release + attestations
  -> GitHub Release exact bundle 재다운로드/검증
  -> 5 platform npm packages publish/verify
  -> @xgen-ai/cli publish/verify
  -> 5-platform public registry install smoke
```

npm publish job은 GitHub-hosted Ubuntu runner와 Node.js 24.20.0을 쓰고, OIDC dist-tag를 지원하는
npm 11.21.3을 고정 sha512로 확인한 뒤 설치한다. npm token은 workflow 어디에도 없다. `id-token: write`의
GitHub OIDC가 npm trusted publishing 인증과 `--provenance` 증명에 함께 쓰인다. Prerelease는 `next`, stable은 `latest` dist-tag로 게시한다. 모든 package에는 npm
provenance가 있어야 하며 registry metadata에서 SLSA provenance predicate가 확인되지 않으면 job이
실패한다.

최초 stable 게시 전에는 모든 package의 SRI·provenance·`next` 검증이 끝난 뒤 launcher의 `latest`도
해당 preview로 갱신한다. 이미 stable 또는 더 높은 version이 하나라도 있으면 갱신하지 않는다.
판정은 pinned `node-semver`로 하며 launcher runtime에는 의존성을 추가하지 않는다.
Platform package는 launcher가 exact version으로 참조하므로 `latest`를 갱신할 필요가 없다.
Alias 갱신만 실패해도 동일 release job 재실행으로 끝낼 수 있고 tarball은 바꾸지 않는다.
GitHub Release의 prerelease/latest 분류는 기존대로다. [ADR-0088](../adr/0088-first-use-preview-distribution.md)을 따른다.

## 부분 실패와 재실행

Platform package를 launcher보다 먼저 게시하므로 중간 실패가 가능하다. 실패한 GitHub Actions job을
같은 run에서 재실행한다. Publisher는 이미 존재하는 exact version의 SRI와 dist-tag가 local immutable
tarball과 모두 같을 때만 skip한다. 하나라도 다르면 자동 수정, unpublish 또는 overwrite하지 않는다.

- 동일 bytes인 일부 게시: 실패 job 재실행
- trusted publisher 미등록·만료·repository/workflow 이름 불일치, launcher dist-tag 권한 누락: npm 설정을 바로잡은 뒤 실패 job 재실행
- registry bytes 또는 package name 소유권 불일치: 중단하고 새 version PR과 tag 사용
- GitHub Release 전 실패: 원인을 main에서 수정하고 더 높은 version/tag 사용

## 로컬 및 PR 검증

실제 registry publish 없이 다음 계약을 확인한다.

```bash
npm ci --ignore-scripts --prefix npm
npm run check --prefix npm
npm test --prefix npm
sh scripts/check-npm-distribution-workflow.sh
```

각 platform CI는 release binary를 platform tarball에 넣고 loopback registry에서 `npm install -g`,
대화형 `/status`/`/exit`, 동일 version 재설치와 `npm uninstall -g`를 수행한다. Release assemble은 여섯
tarball의 file allow-list, 고지와 raw binary byte parity를 확인한다.
로컬이나 PR에서는 실제 npm publish, package bootstrap, GitHub secret 변경과 release tag 생성을 수행하지
않는다.
