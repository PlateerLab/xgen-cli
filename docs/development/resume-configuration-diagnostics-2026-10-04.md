# 재개 설정 불일치 진단 — 2026-10-04

일반적인 진단 개선이다. 미완료 Run 재개가 `configuration_mismatch`로 차단되면 workspace, file catalog,
local execution profile, model request profile 중 어느 검사에서 실패했는지 표시하고 원래 설정을 복원하는
방법을 안내한다. 기존 error code와 exit 64는 유지한다.

## 항목별 비교

새 process-enabled Run의 manifest에는 `processFingerprints`가 선택적으로 추가된다. `executable:ID`는
기존 executable binding과 같은 launch-path digest 및 파일 내용 digest로부터, `environment:KEY`는
기존 safe process environment 값으로부터 SHA-256을 계산한다. raw 경로·환경값·credential을 저장하거나
출력하지 않는다. 개수 128개, 이름 256 bytes, digest 형식과 prefix를 검증하고 manifest record digest에 결합한다.

재개에서 authoritative execution-profile digest가 다르면, 이전 fingerprint와 현재 fingerprint의 추가·제거·변경
항목을 정렬해 JSON 이름 배열로 출력한다. 이 진단 정보로 승인이나 binding을 대체하지 않는다. 검사는 provider
호출과 effect 실행 전에 끝나며 불일치에서는 journal을 변경하지 않는다.

기존 manifest는 필드가 없으면 None으로 읽고 canonical serialization에서 생략하므로 기존 authority digest를
유지한다. 항목 정보가 없는 Run은 실행 profile 범주까지 진단하며 개별 항목 차이를 만들어내지 않는다.
새 manifest는 이전 binary의 strict manifest reader와의 forward compatibility를 보장하지 않는다.

REPL과 headless resume 모두 설정 범주·항목 이름·복구 안내를 전달한다. 실패한 명시적 resume에서 active Run이
있으면 원래 사용자 요청 문맥을 지우지 않는다.

## 이전 실험의 원인 범위

첫 RRSI pilot Run과 후속 profile probe의 model/request profile, workspace identity, file catalog, budget은
같고 local execution profile은 다르다. logical executable 목록도 같다. 과거 Run은 개별 executable/environment
fingerprint를 저장하지 않아, 경로·내용·환경값 중 어느 항목이 달라졌는지는 소급해 확정할 수 없다.
해당 차이를 우회하거나 원본 pilot state를 수정하지 않았다.

## 검증

- 서로 다른 safe environment 항목 LANG, NO_COLOR의 변경을 실제 CLI 재시작 테스트로 재현했다.
  해당 항목 이름이 표시되고 값은 노출되지 않는다. 불일치 상태에서는 SQLite journal bytes가 그대로이며,
  원래 값을 복원하면 완료하고 이미 실행한 read의 receipt는 한 개로 유지된다.
- compiler, runner 두 executable ID에서 파일 내용 변경을 fingerprint가 감지하고 경로를 노출하지 않는지 확인했다.
- 선택적 manifest metadata의 round-trip, 이전 manifest 처리, fingerprint tampering 거절을 검증했다.
- 기존 두 파일 수정·명령 실패·model rejection 뒤 재개 테스트도 유지했다.

자동 재시도, manifest 재작성, binding 완화는 추가하지 않았다. 복구는 원래 설정을 복원한 뒤 명시적 resume이다.
