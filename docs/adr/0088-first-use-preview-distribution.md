# ADR-0088: 기본 preview 설치와 최초 모델 연결 복구

- 날짜: 2026-11-19
- 상태: 채택, rc.7 미게시 실패 뒤 rc.8 게시·공개 설치 검증 완료, rc.9 게시
- 범위: 일반 배포·CLI 표시. Planner 입력·journal·권한·자동 재시도 계약은 바꾸지 않는다.

## 문제와 근거

npm `latest`가 실행 파일 없는 bootstrap을 가리켜 version 없는 설치로 `xgen`을 실행할 수 없었다.
RC6 뒤의 OpenAI parameter style, TUI idle redraw와 Run capability scope도 아직 배포되지 않았다.
온보딩 문서는 OpenAI preset을 이전 `json_schema`로 안내했고, provider/model 번호를 잘못 입력하면
전체 설정이 종료됐다. Model 오류는 reason만 출력해서 다음 조치를 찾기 어려웠다.

이 작업은 최초 사용 흐름의 구현·배포 검증이다. 실제 모델의 장기 작업 완료율을 개선했다고 주장하지 않는다.

## 결정

### 일반: preview의 기본 npm 설치

모든 package는 prerelease를 `next`, stable을 `latest`로 게시하고 SRI·provenance·dist-tag를 검증한다.
첫 stable이 없을 때만, 여섯 package 검증을 모두 마친 뒤 launcher의 `latest`를 해당 preview로 지정한다.
Stable이 하나라도 게시됐거나 더 높은 version이 존재하면 preview로 `latest`를 갱신하지 않는다.
잘못된 registry version, 존재하지 않는 candidate/alias, `next` 불일치는 fail-closed한다.
SemVer 비교는 private npm workspace의 고정 `semver@7.8.6` devDependency로 한다. Launcher runtime과
native binary에는 이 의존성이 들어가지 않는다. 의존성을 설치하지 않는 assembly verifier는 기존
dependency-free 경계를 유지한다. 그 경로의 tag 문법은 build metadata를 허용하지 않는다.

Platform package는 exact optional dependency이므로 기본 alias를 옮기지 않는다. Alias 갱신 실패는
동일 release job 재실행으로 복구하며 tarball을 덮어쓰지 않는다. 기존 release concurrency group으로
publisher를 직렬화한다. 외부 관리자의 동시 dist-tag 변경을 registry 차원에서 원자적으로 막지는 못한다.
GitHub prerelease는 기존처럼 latest release로 지정하지 않으며 native 설치는 exact tag를 사용한다.
기본 npm 설치가 가능해져도 Developer Preview 표시는 유지한다.

### 일반: 모델 설정의 복구 안내

Provider/model 메뉴는 잘못된 번호를 같은 메뉴에서 다시 받는다. `q` 또는 EOF면 기존
`input_unavailable`/exit64로 종료하며 profile commit은 하지 않는다. 메뉴 복구 때문에 catalog나 probe를
추가로 호출하지 않는다. API key 입력·보관 경계는 그대로다.

Model setup/check/configuration 오류는 기존 reason·exit code와 정적 `recovery:` 문장을 함께 출력한다.
인증 재설정, exact model ID, API root, response format·parameter style, provider 가용성·출력/시간 예산을
안내한다. 폐기된 저장 key는 `model logout PROFILE` 뒤 같은 profile을 재설정하도록 안내하며
재설정만으로 기존 key가 교체된다고 가정하지 않는다. Provider 응답 원문·실제 endpoint·key를 오류에 추가하지 않는다. 기존 profile의 옵션은 자동
변경하지 않고 OpenAI profile 갱신 예시를 문서에 제공한다.

README 첫 화면은 일반 설치·모델 연결·작은 작업·복구 순서로 정리하고 현재 preset/TUI 동작과 맞춘다.
문서 검사는 version을 Cargo workspace에서 파생해 수동으로 RC 번호를 맞추는 지점을 없앤다.

## 검증과 한계

- Rust 800 passed, 0 failed, 4 ignored. Clippy·fmt·release build·license·문서/workflow 검사 통과.
- 초기 구현 후보에서 pinned graph-tool-call과 compiled CLI를 사용한 Python 검사 162개 통과(10 skipped). Python 3.14의
  기존 SQLite ResourceWarning은 남지만 실패는 없었다.
- npm 정책 검사 20개(assembly 무의존성 import 회귀 추가): 서로 다른 prerelease 계열, 낮은 stable·build metadata의 hyphen,
  더 높은 preview, 잘못된 metadata, 동일 게시 재실행·SRI/provenance 경계를 검증했다.
- `scripts/smoke-first-use.py`: 격리한 config/state와 실제 CLI에서 provider/model 오입력 뒤 설정 성공,
  실패한 재설정의 기존 profile 보존, compatibility 재검사, Python·JSON 설계 layout과 별도 text layout의
  읽기→patch→쓰기 승인 재개→테스트 승인 재개→완료 replay를 확인했다. 테스트 실행은 각 1회,
  완료 replay의 model 호출은 0회다. 전체 fixture model POST는 14회이며 유료 모델 호출은 없다.
- 같은 smoke는 RC6 control에서 provider 번호 오입력의 `input_unavailable`로 실패했다.
- npm loopback 설치 smoke에서 version 없는 기본 설치·exact 재설치·삭제를 통과했다. 이 로컬 검사는
  host binary를 사용한 launcher fixture이며 native musl 검증은 platform CI에서 따로 한다. PR gate에서 정책
  검사·첫 사용 smoke를 실행한다. 게시 후 공개 registry의 기본 설치도 별도로 확인한다.
- 실제 tmux 결과 viewer·승인 smoke 통과(5 fixture model 호출). 기존 rapid resize stress의 간헐적
  잔상은 이번 작업으로 해결했다고 주장하지 않는다.
- 독립 diff review는 delegate 인증 오류로 실행하지 못했다. Source diff는 로컬에서 직접 검토했다.
- 현재 실제 모델 인증이 없어 유료 모델의 첫 작업 품질 평가는 미실행이다. OS credential store와
  macOS/Windows native terminal의 사용자 실측도 이번 로컬 fixture 검증의 범위 밖이다.

## 배포 결과

rc.7 태그의 source `ca5392b`는 main CI 7개와 release 5개 native build·설치 검사를 통과했지만,
[release run](https://github.com/PlateerLab/xgen-cli/actions/runs/44460752126)의 assembly에서
`ERR_MODULE_NOT_FOUND`로 실패했다. Assembly는 npm 의존성을 설치하지 않는데 verifier에 private
`semver` import를 추가한 실수였다. GitHub Release와 npm package는 게시되지 않았다.
태그는 이동하거나 재사용하지 않고 rc.8로 다시 진행한다.

수정은 verifier의 기존 무의존성 경계를 복원하는 것이다. `semver`는 기존 `npm ci` 뒤에 실행하는
publisher에서만 사용한다. 깨끗한 임시 checkout에서 verifier를 import하는 검사를 추가해 수정 전
실패·수정 후 통과를 확인했다. rc.7의 실제 5개 build artifact를 받아 `node_modules` 없는 checkout에서
launcher까지 6개 package의 assembly verifier를 재생해 통과했다. Artifact 재사용 게시가 아니라
실패 경로의 수정 검증이며 rc.8은 새 source/tag에서 다시 빌드한다.

rc.8은 source `b893d1f`의 [main CI 7개](https://github.com/PlateerLab/xgen-cli/actions/runs/44463791162) 통과 뒤
새 태그로 빌드했다. [릴리즈 workflow](https://github.com/PlateerLab/xgen-cli/actions/runs/44464782757)는
최종 19개 job이 모두 통과했다. 공개 Windows npm 설치의 첫 실행은 optional native package가 없어
실패했지만 같은 source·artifact의 해당 job만 재실행해 설치·버전·고지·REPL·재설치·삭제가 통과했다.
Staged Windows npm 설치는 첫 실행부터 통과했다. Registry 반영 지연 가능성은 있으나 최초 누락
원인은 확정하지 않았고 별도 engine 변경은 하지 않았다.

[GitHub rc.8](https://github.com/PlateerLab/xgen-cli/releases/tag/v0.1.0-rc.8)은 2026-11-19 05:40:55 UTC에
17개 asset, prerelease, `immutable=true`로 게시됐다. npm 여섯 package는 `next=0.1.0-rc.8`,
launcher는 `latest=0.1.0-rc.8`도 확인했다. Platform package의 기본 alias는 옮기지 않는다.

Linux x86-64의 별도 새 HOME·npm cache·prefix에서 version 없는 공개 npm 설치와 exact 재설치를
직접 확인했다. 그 설치본으로 `smoke-first-use.py`의 3개 layout(14 fixture model POST),
`smoke-inline-tui.py --results`(5 fixture model POST)를 통과했다. 공개 GitHub checksum·exact native
installer·npm/native raw binary 일치·`gh attestation verify`도 통과했고 user profile은 변경하지 않았다.
실제 모델 호출·사용자 작업 성공률 평가는 미실행이다. 이 결과를 Claude Code/Codex 수준의 일반 작업
신뢰성 검증으로 해석하지 않는다.

rc.9는 source `d5bea9c`(#162)에서 main CI 7개와 Release run 46141667086의 19개 job이 재실행 없이 통과했다.
[GitHub rc.9](https://github.com/PlateerLab/xgen-cli/releases/tag/v0.1.0-rc.9)는 2026-11-28 12:16:07 UTC에
17개 asset, prerelease, `immutable=true`로 게시됐고 npm 여섯 package는 `next=0.1.0-rc.9`, launcher는
`latest=0.1.0-rc.9`이다.
