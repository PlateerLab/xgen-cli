# 세 언어 프로젝트의 로컬 빌드 검증 — 2026-10-04

결론: 작업 완료와 재개 경계는 12/12에서 유지됐지만, 추가 입력까지 포함한 기능 검증은 10/12,
최종 파일 원문 인용은 8/12였다. 실행 안정성이 최종 설명 정확성이나 모든 입력의 코드 정확성을 보장하지 않는다.
공개 설치판이 아닌 현재 main source의 local release binary를 검증했다. RC3 게시 파일럿 통과로 보고하지 않는다.

## 사전 고정 matrix

Rust, Node.js, Python마다 design 한 사례와 held_out 한 사례를 각각 두 번 실행했다. Rust는 bare REPL,
Node.js·Python은 execute 승인 전 pause한 Run을 새 process에서 resume했다. 프로젝트는 Cargo 표준 crate,
Node.js native test, Python unittest로 구성하고 외부 dependency 설치 없이 기존 논리 오류를 수정하도록 했다.
Model·공유 prompt·engine 코드는 변경하지 않았다. Fixture source, visible/hidden tests, binary·runner hash와
반복 수는 실행 전에 고정했다. 12회 모두 DeepSeek deepseek-flash를 사용했다.

| 언어 | 실행 | 독립 visible test/build | 추가 경계까지 통과 | 파일 원문 인용 | Model의 실패 process 관찰 | model calls |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Rust | 4 | 4/4 | 4/4 | 3/4 | 4/4 | 25 |
| Node.js | 4 | 4/4 | 2/4 | 3/4 | 4/4 | 29 |
| Python | 4 | 4/4 | 4/4 | 2/4 | 2/4 | 28 |

각 언어 두 기능을 두 번 반복한 작은 표본이다. Rust는 기본 8-turn budget, 다른 모드는 24-turn budget을
사용해 언어별 성능 비교가 아니다. 실행 중앙 시간은 Rust 10.29초, Node.js 11.20초, Python 9.26초였다.

## 완료·재개·사용량

- 12/12가 durable task completion과 최종 응답을 저장했다. 기존 테스트·설정 파일은 모두 보존됐다.
- Node.js·Python 8/8에서 execute 승인 전 pause, process output 0건을 확인했다. 같은 Run을 별도
  process에서 승인해 완료했다. 상태를 새 작업으로 바꾸거나 binding을 우회하지 않았다.
- 12/12가 workspace/model 없이 완료 응답을 정확히 replay했고 journal·usage bytes는 그대로였다.
- 정확한 protocol event 이름으로 별도 재검사한 Unknown과 중복 effect starts는 모두 0이었다.
- 82 model calls의 usage coverage는 모두 완전했다. 총 479702 tokens와 cached input 329344 tokens를
  관측했다. cache는 입력의 일부이므로 전체에 다시 더하지 않는다. 금액 단가를 주지 않아 비용은 계산하지 않았다.
- 같은 binary source의 idle/model-call SIGINT regression 2개가 통과했다. 실제 API 호출 중 강제 중단이나
  process tree leak은 이번 실험에서 측정하지 않았으며 자동화 회귀와 실사용 관측을 구분한다.

## 확인된 한계

Node.js query parsing의 두 실행은 visible tests와 구문 검사를 통과했지만 hidden 경계에 실패했다.
두 번 모두 leading `?`를 놓쳤고, 두 번째는 bare key와 value 안의 `=`도 놓쳤다. URLSearchParams 같은
표준 API를 쓰면 처리할 수 있는 경계다. 그러나 이 요구를 visible 명세에 모두 제공하지 않았으므로 기존
명세 위반이나 engine 결함으로 단정하지 않는다. 다음 평가에는 구현 방식 대신 필요한 입력 계약을 명시해야 한다.

파일 원문 인용은 네 번 달랐다. Rust의 `!= 0`을 `> 0`으로 바꾼 것과 Python transpose 재구현 두 번은
fixture 범위에서 동작이 같아도 literal 요청은 충족하지 않는다. Node.js 한 번은 실제 파일의 첫 `=` 뒤
재결합을 제거한 코드를 보여줘 추가 경계에서 동작까지 달라졌다. 실제 저장 파일·최종 sidecar와 대조했다.
자연어 전체의 거짓 주장 판정이나 모든 입력의 의미적 동등성 검증으로 일반화하지 않는다.

Python 두 번은 초기 failing tests를 runner가 확인했지만 model의 실패 process output 관찰은 없었다.
따라서 “모델이 항상 실패를 직접 보고 고쳤다”는 결론도 내리지 않는다. 반복 결과는 삭제하거나 재실행하지 않았다.

## 측정 도구와 재현

[evaluation 안내](../../evals/projects/README.md)에 실행 command와 원시 report, 수동 검토,
실행 시점 source snapshot을 보존했다. 원본 private state/workspace는 `/tmp/xgen-project-acceptance-vlm2zsrt`다.
초기 runner의 Unknown event 문자열 두 개가 protocol과 달라, 정확한 이름으로 12개 journal을 다시 검사했다.
원시 report를 바꾸지 않고 별도 audit를 남겼다. 이후 runner는 event 이름·중복 starts와 실패/timeout의 부분
journal 관측과 fixture/hidden 경로 검증을 보완했다. API를 재실행하지 않았으며 engine 개선이나 비용 절감 실험으로 보고하지 않는다.

Python 평가 테스트 21개, compilation과 diff 검사를 통과했다. SIGINT 회귀 2개도 통과했다.
다음 순서는 명시적인 입력 계약이 있는 새 사례로 기능·최종 응답 평가를 확장하는 것이다. 자동 재시도나
사례별 engine 규칙을 추가하지 않았다. 공개 설치·온보딩·게시 검증은 여전히 별도 작업이다.
