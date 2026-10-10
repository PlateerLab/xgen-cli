# 오픈소스 검색·터미널 backend 검증

## 결과와 적용 상태

검색은 **OpenSERP를 다음 연결 구현의 우선 후보**로 정했다. 검색 결과 반환과 공식 출처 포함은
DDGS보다 안정적이었지만, 기존 process 경로에 연결한 live 검증은 2건 중 1건만 전체 gate를 통과했다.
기본 검색 도구를 활성화하거나 자동 설치하지 않았다.

터미널은 **portable-pty를 기반 라이브러리 후보**로 유지한다. 입력·출력·크기 변경은 동작하지만
`Child::kill()`만으로 하위 process 정리가 되지 않는 문제를 서로 다른 두 구조에서 재현했다.
소유한 process group 종료는 두 구조 모두 통과했다. 이번 구현은 dev-dependency와 offline probe이고,
현재 shell 없는 bounded process Capability를 PTY로 교체하지 않는다.

결과 파일은 [evals/tool-backends/results/2026-10-05](../../evals/tool-backends/results/2026-10-05)에 있다.
기존 agent loop·승인·Receipt·복구 계약을 유지한다. 기본 사용자 설정 변경은 없다.

## 후보와 재사용 범위

| 후보 | 라이선스 | 재사용 범위 | 설치·실행 부담 |
|---|---|---|---|
| [DDGS](https://github.com/deedy5/ddgs) | MIT | Python library·CLI·HTTP API·MCP | Python과 별도 dependency 필요 |
| [OpenSERP](https://github.com/karust/openserp) | MIT | Go CLI·HTTP API·본문 추출 | 이번 DuckDuckGo 경로는 Chrome/Chromium 필요 |
| [SearXNG](https://github.com/searxng/searxng) | AGPL-3.0 | 외부 검색 서비스의 JSON API | 별도 서비스 운영 필요; live 비교에는 포함하지 않음 |
| [portable-pty](https://docs.rs/portable-pty/latest/portable_pty/) | MIT | Rust PTY primitive | 세션 관리·출력 보관·process tree 정리는 host 책임 |
| [node-pty](https://github.com/microsoft/node-pty) | MIT | Node PTY primitive | Rust CLI에 Node 실행 환경이 추가되므로 우선순위 낮음 |

[Gemini CLI shell 실행부](https://github.com/google-gemini/gemini-cli/blob/main/packages/core/src/services/shellExecutionService.ts)는
세션·출력·중단 처리를 참고할 수 있다. 설정, sandbox, lifecycle와 결합된 구현이므로 독립적인 Rust
도구 모듈로 그대로 가져올 수 있다고 판단하지 않았다. 라이선스는 Apache-2.0이다.

## 검색 비교

- DDGS 9.16.0, OpenSERP v0.8.12 Linux x86-64 release, 설치된 Google Chrome 사용.
- 영어 4개·한국어 4개 질의 × backend 2개 × 반복 2회 = 32회.
- 설계 질의 2개와 held-out 질의 6개를 분리했다. 두 backend에 같은 질의와 결과 상한 5개를 적용했다.
- DDGS는 `auto`, OpenSERP는 DuckDuckGo browser mode다. **동일 검색엔진만의 비교가 아니다.**
- 실행 동시성 2, 외부 wall-clock 상한 20초. OpenSERP retry 0; runner는 재시도하지 않는다.
  DDGS `auto` 내부의 backend 선택·fallback은 라이브러리 동작이다.
- 최종 runner는 `waitid/WNOWAIT`로 leader 종료를 관찰하고, 소유 group을 정리한 뒤 leader를 reap한다.
  종료된 PID가 정리 전에 재사용되는 순서를 피한다. 이 보완 후 별도 4회 검색 검증을 수행하며 최초 32회와
  cohort를 섞지 않는다. 정상 exit와 deadline 양쪽의 unreaped cleanup을 offline 테스트한다.
- 질의는 공개 정보만 사용했다. 검색 process에는 API key·credential·전체 ambient environment를 넘기지 않았다.
- 성공은 정상 exit·JSON schema·HTTP(S) URL·nonempty 결과를 기준으로 한다. 별도로 설정한 공식 도메인이
  상위 5개에 포함되는지 측정했다. 본문 내용, 최신성, 응답 전체의 사실 정확성은 이 gate에 포함하지 않는다.

| 지표 | DDGS | OpenSERP |
|---|---:|---:|
| 정상 nonempty 응답 | 11/16 | 16/16 |
| 공식 도메인 top-5 포함 | 11/16 | 16/16 |
| 영어 정상 응답 | 5/8 | 8/8 |
| 한국어 정상 응답 | 6/8 | 8/8 |
| held-out 공식 도메인 top-5 포함 | 8/12 | 12/12 |
| 평균 wall-clock | 3,589 ms | 1,848 ms |

DDGS의 5회 실패는 `No results found` exception이었다. 성공한 회차만 골라 평균을 계산하지 않았다.
OpenSERP의 raw HTTP 모드로 DuckDuckGo를 호출한 사전 확인은 지원되지 않아 실패했다. 이때 browser mode로
전환했고 본 실험 32회 전에 설정을 고정했다. Chrome이 이미 설치된 환경의 결과이며, 신규 환경의 설치
시간·용량과 다른 OS에서의 안정성을 측정한 결과는 아니다.

SearXNG의 public instance는 JSON output이 비활성화될 수 있다.
[공식 API 문서](https://docs.searxng.org/dev/search_api.html)에 따라 자체 instance 설정과 운영을 별도 과제로
보았으며, 설치가 단순한 사용자 흐름을 우선 검토하는 이번 비교에서는 실행하지 않았다.

## xgen process 연결

별도의 agent loop 없이 OpenSERP를 기존 executable catalog의 `web-search` logical ID로 연결했다.
비교에 쓰지 않은 영어 curl 문서 질의와 한국어 Python ast 문서 질의를 각각 fresh workspace·Run에서 실행했다.
모델은 기존 default profile의 DeepSeek를 사용했다. API 호출 비용은 발생하지만 key는 secure store에서 사용했고
결과 파일에는 credential과 private Run store를 넣지 않았다.

gate는 실행 승인 전 process output 0개, 승인 후 단일 search 실행·Receipt, 최종 응답의 제목·URL이
관찰한 첫 두 결과와 exact match, offline replay에서 journal·usage 변경 없음, unknown·중복 effect start 0개다.

- 영어 curl 사례: 모든 gate 통과. model call 2회, tool effect·Receipt 1개, 출처 exact match와 offline replay 확인.
- 한국어 Python ast 사례: `proposal_rejected.invocation_invalid`로 실행 전 중단. 검색 backend 실패로 분류하지 않는다.
  모델이 실제로 어떤 잘못된 invocation을 제안했는지는 보존된 거부 사유만으로 확정할 수 없다.
- 새 cohort의 통과율은 **1/2**다. 기본 도구 채택 gate를 통과했다고 주장하지 않는다.
- 최초 smoke runner가 `reason=` 대신 `code=`를 검사했다. 첫 cohort는 계측 오류가 있는
  `integration-invalid-runner-report.json`로 보존하고, 수정한 runner의 fresh cohort와 섞지 않았다.
  자동 retry로 실패를 성공으로 대체하지 않았다.
- 결과 생성 뒤 smoke runner에는 초기 진단·안전 지표 보존, 실패 분류, 실패 시 nonzero exit를 추가했다.
  search 오류 문구·종료 순서와 PTY formatting도 정리했다. 실행 당시와 최종 script가 byte-identical이라는 주장은 하지 않는다.
  backend release 및 실행한 xgen binary digest는 결과에 남겼다.

연결 smoke는 복잡한 argv를 모델이 구성하는 기존 generic process 경로의 가능성과 한계를 확인한다.
검색 질의만 받는 typed Capability나 설치 자동화까지 구현한 것은 아니다.

## PTY 검증

`crates/xgen-adapter-process/examples/pty_backend_probe.rs`를 Linux에서 실행했다.
모델·검색·API key 없이 fixture process만 사용한다. CI Quality / Linux에도 같은 probe를 추가했다.

| 사례 | 결과 |
|---|---|
| 영어·한국어 입력, PTY 크기 변경, shell metacharacter의 literal argv 전달 | 2/2 통과 |
| 약 120 KiB 출력의 계속 drain, 보존 상한 1 KiB | 통과 |
| exit code 7 보존 | 통과 |
| SIGHUP 무시 자식·중첩 자식에 `Child::kill()`만 적용 | 2/2에서 지연 marker 생성 |
| 같은 두 구조에 소유한 process group SIGKILL 적용 | 2/2에서 지연 marker 없음 |

지연 marker가 만들어지는 시간을 지난 뒤 결과를 관찰하고 test 소유 group을 정리했다.
이는 session/process group 밖으로 빠져나간 hostile process를 막는 sandbox 검증이 아니다.
probe는 Linux의 `waitid/WNOWAIT`로 일반 child의 PID를 cleanup까지 유지한다. 라이브러리 `Child::kill()`은
내부에서 leader를 reap할 수 있어, 이 실패 재현 fixture의 descendant는 marker 이후에도 살아서 group을 유지하게 했다.
macOS와 Windows Job Object·ConPTY lifecycle는 검증하지 않았다. Linux 이외에서는 probe가 `not_validated`를 반환한다.
현재 probe의 bounded raw bytes는 terminal rendering이나 UTF-8 chunk 처리 계약을 구현한 것이 아니다.

## 다음 구현 순서

모두 **일반 개선**이다. 특정 질의·문서의 이름이나 업무 기준을 engine prompt에 추가하지 않는다.

1. 검색: `query`와 bounded result count만 받는 typed adapter를 설계하고, OpenSERP 실행·실패·결과 schema를
   host가 처리한다. 모델이 backend의 긴 CLI argv를 구성하지 않아도 되게 한다. 고정 결과 fixture 두 종류와
   새 held-out live 질의에서 승인·출처·replay gate를 다시 확인한다.
2. 설치: backend·browser 탐지와 설치를 한 진입점으로 묶는다. 버전·digest·dependency·설치 실패를 확인한 뒤
   opt-in 활성화한다. 사용자가 환경변수나 별도 서버를 일일이 설정하는 흐름을 기본으로 만들지 않는다.
3. 터미널: host 소유 `start/read/write/cancel` 세션 계약, bounded output, deadline과 process tree 정리를 구현한다.
   Windows Job Object·ConPTY를 별도로 검증한다. 여러 tool call 사이의 같은 process 유지와 host 재시작 후
   세션 소실 의미를 고정한다. 복구 때 명령을 자동으로 다시 시작하지 않는다.
4. 두 종류 이상의 설계 사례와 별도 held-out 사례에서 검증한 뒤 production Capability 등록과 기본 UX를 연결한다.
