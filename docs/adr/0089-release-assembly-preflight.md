# ADR-0089: Native build 전에 실행하는 공통 조립 검사

- 날짜: 2026-11-20
- 상태: 채택, 구현·로컬 검증 완료
- 범위: 일반 배포 검증. CLI runtime·권한·journal·모델 입력은 바꾸지 않는다.

## 근거와 결정

rc.7은 5개 native build 뒤 assembly verifier의 `semver` import 누락으로 실패했다.
rc.8에 추가한 무의존성 import 검사는 module load만 확인하므로 함수 실행 중의 의존성 누락은 못 잡는다.
따라서 실제 pack·검증·checksum 생성을 native compiler보다 먼저 실행한다.

`npm/scripts/assemble-release.mjs`를 production과 검사 양쪽의 공통 entrypoint로 둔다.
입력 native asset·platform tarball 목록은 `npm/platforms.json`에서 파생한다. Launcher pack,
원본 계약·binary byte parity 검증, installer·고지 복사와 checksum 생성을 같은 코드로 수행한다.
입력·출력 집합과 regular-file 조건을 검사하고 덮어쓰지 않는다. `checksums.sha256`은 기존 형식을
유지하되, 예전 shell glob이 중복 나열하던 tgz를 포함해 각 파일을 한 번만 기록한다.

`npm/scripts/preflight-release.mjs`는 공개 source 중 필요한 파일만 새 임시 환경에 복사한다.
`node_modules`·user config를 복사하지 않고 npm cache·HOME·설정을 별도로 둔다. 환경변수는 toolchain에
필요한 항목만 허용하며 npm은 offline·ignore-scripts로 실행한다. Compiler·model·registry 게시·token은
필요하지 않다. 두 fixture version(stable/prerelease)에서 실제 packer와 공통 entrypoint를 호출한다.
Payload는 `ASSEMBLY_FIXTURE_ONLY` byte이며 native 프로그램처럼 실행하거나 artifact로 업로드하지 않는다.

Fast/Linux는 Node.js 설치 직후 Rust toolchain·npm ci 전에 이 검사를 수행한다. Release quality에서도
npm ci·compile 전에 수행하고 native jobs는 기존대로 quality 성공을 기다린다. Workflow 계약 검사는
이 순서와 공통 entrypoint 연결을 확인한다. 실제 native inspection·실행·installer 검사는 유지한다.

## 검증과 한계

- 로컬 전체 preflight는 약 7~9초였다. 두 version의 6개 package 조립·17개 asset·checksum 전체 포함과
  중복 부재를 확인했다. 추가 파일·raw/package 불일치·마지막 platform tarball 누락은 각 version에서 거절했다.
- 임시 source에서 static `semver` import를 재삽입하자 1.2초에 거절했다. 함수 호출 시 `import('semver')`를
  추가한 경우 기존 import-only 검사는 통과했지만 새 preflight는 1.4초에 거절했다. 이 변경은 제품 source에 남기지 않았다.
- 설계에 쓰지 않은 공개 rc.8의 실제 5개 native asset·5개 platform package를 SHA-256으로 확인한 뒤,
  `node_modules` 없는 checkout에서 새 assembler로 조립했다. 17개 결과 중 checksum manifest를 제외한
  모든 파일이 게시본의 SHA-256과 같았다. 새 manifest도 각 파일의 실제 hash와 일치했다.
- 그 bundle의 실제 Linux binary·복사한 installer로 기존 설치·재설치·손상 checksum 거절·symlink 거절·
  protocol·license·REPL smoke가 통과했다. npm20개와 workflow·문서 검사도 통과했다.
- 독립 diff 검토 도구는 인증 오류로 실행하지 못해 직접 검토했다.

이는 packaging 경로를 앞에서 검사하는 변경이다. 가짜 byte로 CPU/OS ABI나 실제 native 실행을 검증하지
않으며, 전체 CI·release 시간이 단축됐다는 측정도 아니다. npm registry 반영 지연과 Windows optional
package 누락 원인은 별도 문제로 남는다. 이번 변경을 위해 새 release tag나 package를 게시하지 않는다.
