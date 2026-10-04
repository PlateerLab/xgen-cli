# 모델 응답 거절 이후의 사용자 안내 — 2026-10-04

파일 수정과 테스트 실행 뒤 모델의 마지막 응답이 거절되면, 기존 REPL은 오류 코드만 표시하고 현재 Run과
요청 문맥을 해제했다. 사용자가 작업 완료와 답변 생성 실패를 구분하기 어렵고, 명시적으로 재개해도 이후 대화에
원래 요청을 연결하지 못하는 문제가 있었다.

## 확인한 원인과 변경

첫 RRSI pilot의 `windows` 후보 첫 실행은 execution receipts 7개와 tool outputs 7개를 저장했지만
completion output은 없었다. 마지막 호출은 `planner_invalid_response`로 durable하게 거절됐다.
원문 모델 응답을 보존하지 않으므로 JSON 문법·필드·응답 envelope 중 무엇이 원인인지는 확인할 수 없다.

일반적인 REPL 상태 관리 변경이다. `ModelRejected`에서 active Run과 원래 요청을 유지하고 다음을 표시한다.

- 최종 답변이 저장되지 않았음.
- 이미 commit한 실행 결과는 유지되지만 작업 완료는 확정되지 않았음.
- `/resume RUN_ID`로 명시적 재개, `/clear`로 다른 작업 시작.

재개 또는 clear 전까지 다른 요청은 기존 active Run 보호 규칙에 따라 차단한다. 같은 Run을 재개해 완료하면
원래 요청과 최종 응답이 함께 후속 대화 문맥에 들어간다. 다른 Run 재개와 clear의 기존 문맥 분리도 유지한다.
원래의 승인·model-call reservation·Unknown 복구·budget·Receipt 계약은 변경하지 않는다.

## 검증

- REPL unit tests 19개 통과. Invalid response와 output truncation 두 종류에서 명시적 resume만 발생하며,
  성공 뒤 원래 요청을 유지하고 clear 뒤에는 실패한 요청이 섞이지 않는지 확인했다.
- 실제 CLI/HTTP/SQLite integration tests 6개 통과. 새 시나리오는 두 파일 저장, 실패하는 명령 실행, 마지막
  응답 거절, CLI 종료·재시작, 명시적 resume, 완료 응답 offline replay를 연결한다. Empty summary와 잘못된
  format version 두 입력에서 execution receipts 3개가 전후 정확히 같고 추가 effect가 시작되지 않았다.
- Clippy `-D warnings`, release build 통과. 기존 `~/.local/bin/xgen` symlink가 새 release build를 가리킨다.

이전 DeepSeek 실험 Run은 별도 state 복사본으로 재개를 확인했지만 `configuration_mismatch`에서 중단됐다.
새 manifest와 비교한 model·request-profile digest·workspace identity·file catalog·budget은 같고,
local execution profile digest가 달랐다. 정확히 어떤 process catalog 요소가 달라졌는지는 확인하지 못했다.
catalog binding을 우회하지 않았고, 이 실행을 실제 DeepSeek 복구 성공으로 보고하지 않는다.
원본 workspace와 pilot state는 그대로 보존했다. 복사본에서도 receipts·tool outputs·completion outputs가
변하지 않았고 새 effect 권한은 허용하지 않았다.

재개 확인: `/tmp/xgen-final-response-recovery-1vsfselp/report.json`.
설정 비교: `/tmp/xgen-resume-profile-probe-y35gmwi7/comparison.json`.
