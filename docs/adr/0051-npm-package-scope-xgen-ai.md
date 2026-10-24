# ADR-0051: npm package scope는 `@xgen-ai`를 쓴다

- 상태: Accepted
- 날짜: 2026-10-24
- 적용 범위: npm package 이름, GitHub Actions release, 사용자 설치 문서
- 대체 결정: ADR-0031과 ADR-0046의 `@xgen` scope 사용
- protocol·journal·SQLite schema 변경: 없음
- runtime 구현 변경: launcher가 찾는 platform package 이름

## 배경

ADR-0031은 launcher 하나와 platform package 다섯 개를 `@xgen` scope에 게시하기로 했고, ADR-0046은
제품 이름을 XGEN으로 통일하면서 이 scope는 이미 XGEN 이름이므로 유지한다고 적었다. 두 결정 모두 그
scope를 쓸 수 있다는 전제를 확인하지 않았다.

2026-10-24 실측 결과 `@xgen`은 다른 계정이 이미 선점하고 있다. 그 아래 게시된 package는 없지만, npm은
user와 organization이 같은 namespace를 공유하므로 organization으로도 확보할 수 없다. ADR-0031에서 파생된
운영 절차는 "scope 확보 여부가 불명확하면 게시하지 말고 naming ADR을 먼저 갱신한다"로 fail-closed를
걸어 두었다. 따라서 이름을 바꾸는 결정이 게시의 선행 조건이다.

## 결정

npm scope를 `@xgen-ai`로 바꾼다. 게시할 여섯 package는 다음과 같다.

- `@xgen-ai/cli`
- `@xgen-ai/cli-linux-x64-musl`
- `@xgen-ai/cli-linux-arm64-musl`
- `@xgen-ai/cli-darwin-x64`
- `@xgen-ai/cli-darwin-arm64`
- `@xgen-ai/cli-win32-x64`

ADR-0031이 정한 배포 구조는 그대로다. 같은 exact version, lifecycle script 없음, 설치 중 download와
native compile 없음, provenance는 OIDC 증명이다. 바뀌는 것은 이름 하나다.

`@xgen-ai`를 고른 이유는 회사의 공개 배포 식별자가 이미 그 이름이기 때문이다. 다른 registry에서도 같은
이름으로 모델과 산출물을 공개하고 있어, 설치 경로와 공개 식별자가 어긋나지 않는다.

개인 계정 scope는 쓰지 않는다. 제품 설치 경로가 개인 식별자를 가리키면 소유권을 조직으로 옮길 때 이미
게시된 package 여섯 개를 deprecate하고 사용자에게 재설치를 요구해야 한다. 이름은 게시 이후 사실상
되돌릴 수 없으므로 첫 게시에서 결정한다.

비어 있던 다른 이름(`@xgencli`, `@xgen2bee`)은 제품 이름도 회사 식별자도 정확히 가리키지 못해
버렸다. Scope 없는 `xgen` 단일 package는 platform package 다섯 개를 묶을 namespace가 없어 ADR-0031의
구조와 맞지 않는다.

## 결과

`@xgen` scope에 게시된 XGEN package는 없다. rc.3이 최초 게시이므로 이름 변경에 따르는 사용자 마이그레이션
경로가 필요하지 않다.

변경은 운영 자산에만 적용한다. `npm/platforms.json`과 `npm/packages/cli/`의 package 정의, release
workflow, 검사 script, 사용자 설치 문서다. ADR-0031·ADR-0034·ADR-0046 본문은 당시 결정의 기록이므로
`@xgen`을 그대로 두고 상태 줄에서 이 결정을 가리킨다.
