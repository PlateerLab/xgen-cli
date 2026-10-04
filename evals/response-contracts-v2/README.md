# 최종 응답 2차 실험

1차의 별도 검증 사례였던 `split-last`와 `rotate`를 설계 사례로 옮겼다. 새 별도 검증 사례는 `normalize-lines`와 `adjacent-dedupe`다. 입력 계약은 두 조건에 동일하게 제공하고 visible·hidden 검증은 계약 안의 입력만 사용한다.

후보 지시는 1차의 `../response-contracts/copy-observation.txt`를 그대로 사용한다. 두 조건 모두 소스 코드와 명령·exit code·변경 파일의 JSON을 최종 응답에 요구한다. JSON 검사는 자유 형식 설명 전체의 사실성을 증명하지 않는다. 전체 주장에 대한 확대 해석을 피하고, 답변 예시와 실제 기록은 결과 검토에서 대조한다.

```sh
python3 scripts/experiment-response-contracts.py \
  --binary target/release/xgen \
  --cases evals/response-contracts-v2/cases.json \
  --require-evidence --attempts 3 --timeout 240
```

이 명령은 실제 model API를 사용한다. 실행 전 입력 SHA256과 판정 기준을 등록하고 24회 종료 뒤 재확인한다. 기존 실패 사례를 설계에 사용했으므로 그 사례의 개선만으로 일반화를 주장하지 않는다. 후보나 새 검증 사례를 결과에 맞춰 수정하거나 실패를 재시도하지 않는다.

채택 조건은 서로 다른 설계 사례 두 개의 기존 인용 실패 재현, 양 split에서 인용 개선, workflow·인용·근거 응답 회귀 없음, 별도 검증에서 근거 응답 개선, 각 split 평균 호출과 total tokens 증가 25% 이하다. 이 조건을 통과해도 기본 적용 전에 더 큰 검증이 필요하다.

명령 근거는 완료된 process output을 해당 step의 invocation recipe와 연결해 argv와 exit code를 가져온다. 계획만 있고 실행되지 않은 명령은 세지 않는다. 모든 실행을 순서대로 비교하므로 실패·반복 실행 누락도 잡는다. 변경 파일은 작업 전후 내용 비교로 생성·삭제를 포함하며 Python runtime cache만 제외한다. source 인용 검사는 앞뒤 whitespace를 제외하고 전체 내용 일치를 요구한다.
