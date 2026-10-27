# ADR-0073: 출력 상한에 걸린 응답도 버리고 다시 받는다

- 상태: Accepted
- 기준일: 2026-10-27
- 변경 분류: 범용 driver 재시도. protocol·journal·wire 순서 변경 없음.

## 문제

긴 이름 변경 작업에서 Run 두 개가 `model_rejected.output_truncated`로 끝났다(H6-c1, H7-w1). 두 응답 모두
`apply-patch` 하나에 같은 edit를 254~439번 반복하다가 출력 상한(16,384 token)에 닿았다. 반복된 `oldText`는
파일에 한 번씩만 있는 줄이었으므로 모호한 patch 문맥 문제가 아니라 모델의 반복 생성이다. 응답 하나에 약 45초가
들고, Run은 ADR-0061 재시도 대상이 아니어서 그대로 멈췄다.

기록된 응답 4,402개 중 상한에 닿은 것은 이 2개뿐이다. 정상 응답의 최대 길이는 3,397 token이고 2,048 token을
넘은 것은 1개다.

## 결정

1. Driver는 `PlannerPortFailure::OutputTruncated`를 `InvalidResponse`와 같이 연속
   `MAX_REJECTED_RESPONSE_RETRIES`(2)번까지 버리고 새 model call을 한다. 연속 횟수와 예산은 ADR-0061 그대로다.
2. 재시도 context의 `previousResponseRejection.meaning`은 고정 문장으로 "출력 상한에 닿아 버렸다. 더 짧게,
   edit는 한 번씩"을 알린다(ADR-0064 방식). 원래 큰 출력이 필요한 제안이 같은 이유로 세 번 잘리면 지금처럼
   멈춘다.
3. Provider 한도, 요청 크기, rate limit은 다시 보내도 결과가 같으므로 계속 재시도하지 않는다.

출력 상한(profile `max_output_tokens`)은 바꾸지 않는다. 정상 응답의 최대가 3,397 token이라 낮추면 반복 한 번의
비용은 줄지만, profile 값이고 큰 새 파일 작성 같은 정당한 긴 출력과 구분할 근거가 아직 없다.

## 검증

- 두 잘린 요청을 DeepSeek에 그대로 다시 보내면 2/2가 203·237 token의 정상 제안이었다(edit 2~3개, 중복 없음).
  거절 이유를 넣은 요청도 4/4가 199~201 token의 정상 제안이었다(합계 6/6). 커밋 메시지의 7/7은 6/6의 오기다. 반복은 같은 입력에서 재현되지 않는 드문 표본이다.
- 반복이 드물어 실행 A/B로는 재현할 수 없다. e2e 테스트: 첫 응답이 `finish_reason=length`면 두 번째 요청이
  `previousResponseRejection.reason=output_truncated`와 고정 문장을 담고 Run이 정상 종료한다. driver 변경을 되돌리면
  이 테스트가 실패한다.
