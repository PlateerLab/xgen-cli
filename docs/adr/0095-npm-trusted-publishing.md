# ADR-0095: npm 게시를 trusted publishing(OIDC)으로 옮긴다

- 날짜: 2026-11-29
- 상태: 채택. workflow 변경 구현, npm 쪽 trusted publisher 등록과 실제 게시 검증은 다음 rc에서 한다.
- 대체: [ADR-0034](0034-npm-token-publishing-policy.md)의 granular token 인증 결정. provenance·SRI·launcher-last
  검증은 그대로 둔다.
- 범위: release workflow의 `publish-npm` job, 운영 문서와 workflow 검사. CLI runtime, Planner 입력, journal은
  바꾸지 않는다.

## 문제와 근거

ADR-0034는 계정 2FA와 trusted publisher 설정이라는 사람 개입을 피하려고 `bypass 2FA` granular token을
골랐다. npm은 이 전제를 없앤다.

- npm access token 문서(2026-09-10 수정): bypass-2FA token의 직접 게시 권한은 2027년 1월에 제거된다.
  이후 token은 `npm stage publish`로 staging만 할 수 있고 최종 게시는 maintainer의 2FA 승인이 필요하다.
  token을 유지하면 release마다 사람이 승인해야 한다.
- GitHub changelog(2026-09-30): trusted publishing 설정마다 opt-in `Allow npm dist-tag` 권한이 생겼다
  (npm CLI 11.21.0 이상). 이전에는 OIDC로 `npm dist-tag add`를 할 수 없어서 launcher `latest` 갱신
  ([ADR-0088](0088-first-use-preview-distribution.md))에는 token이 필요했다.

trusted publishing은 package마다 한 번 2FA로 등록하면 이후 release는 사람 개입 없이 끝난다. token 탈취
위험도 사라진다.

## 결정

1. `publish-npm` job은 npm token을 받지 않는다. `secrets.NPM_TOKEN`, `NODE_AUTH_TOKEN`,
   `actions/setup-node`의 `registry-url`(token 참조 npmrc를 만든다)을 workflow에서 뺀다. 기존
   `id-token: write`가 이제 npm 인증과 provenance 둘 다에 쓰인다.
2. Node 24.20.0에 들어 있는 npm 11.19.0은 OIDC dist-tag를 지원하지 않는다. job은 npm 11.21.3 tarball을
   registry에서 받아 workflow에 고정한 sha512 integrity와 비교한 뒤 global 설치하고, 게시 step에서 version을
   다시 확인한다.
3. `publish.mjs`는 바꾸지 않는다. `--provenance`, launcher-last, SRI·dist-tag·provenance 사후검증,
   동일 bytes 재실행 skip을 그대로 쓴다.
4. 여섯 package 모두 trusted publisher를 `PlateerLab/xgen-cli`, workflow `release.yml`, environment 없음으로
   등록한다. launcher `@xgen-ai/cli`에만 `Allow npm dist-tag`를 켠다(`latest` 승격을 하는 유일한 package).
5. `XGEN_NPM_PUBLISH_ENABLED=true`는 이제 "여섯 package에 trusted publisher가 등록됐다"는 관리자
   확인이다. 값이 없으면 native build 전에 release가 실패한다.
6. 첫 OIDC 게시가 검증된 뒤에 package 설정을 "Require two-factor authentication and disallow tokens"로
   바꾸고 repository secret `NPM_TOKEN`과 npm token을 폐기한다. 검증 전에는 되돌릴 수 있게 남긴다.

## 대안

- stage-only token + release마다 2FA 승인: 자동 release가 사라진다. 불채택.
- token으로 계속 직접 게시: 2027년 1월 이후 동작하지 않는다.
- setup-node의 Node version을 올려 npm을 바꾸기: OIDC dist-tag를 지원하는 npm이 들어 있는 Node 24 release가
  아직 없다(24.22.3에 npm 11.19.1). Node가 따라오면 이 단계를 없앤다.

## 검증

- `scripts/check-npm-distribution-workflow.sh`: workflow 전체에 `NPM_TOKEN`·`NODE_AUTH_TOKEN`·`_authToken`·
  `registry-url:`가 없고, 고정 npm 설치 step에 version·integrity 비교가 있으며 bundle 검증이 게시보다 먼저인지
  본다.
- 로컬: 고정 integrity와 registry tarball의 sha512가 일치했고, 설치한 npm 11.21.3에 `npm trust`가 있다.
- 남은 것: 실제 OIDC 게시와 launcher `latest` 갱신은 trusted publisher 등록 뒤 다음 rc release에서 확인한다.
  실패하면 GitHub Release는 이미 immutable이므로, 등록을 바로잡은 뒤 `publish-npm`만 재실행한다.

## 운영 절차 (npm 관리자, 2FA 필요)

```bash
# npm 11.15 이상 필요. 로컬 npm이 낮으면 npx로 고정 version을 쓴다.
for p in cli-linux-x64-musl cli-linux-arm64-musl cli-darwin-x64 cli-darwin-arm64 cli-win32-x64 cli; do
  npx --yes npm@11.21.3 trust github "@xgen-ai/$p" \
    --file release.yml --repository PlateerLab/xgen-cli --allow-publish --yes
done
npx --yes npm@11.21.3 trust list @xgen-ai/cli
```

그다음 npmjs.com의 `@xgen-ai/cli` trusted publishing 설정에서 `Allow npm dist-tag`를 켠다. 새 설정은
2일 안에 첫 게시가 성공하지 않으면 만료되므로 등록은 release tag 직전에 한다.
