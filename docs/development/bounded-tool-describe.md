# 제한된 모델 조회와 전체 실행 계약

일반 기능이다. 큰 명세를 모델 입력 한도에 맞추면서 실행 검증은 전체 원본으로 수행한다. 검색·색인·순위 알고리즘과 graph-tool-call 0.46.0은 변경하지 않는다.

## 사용과 계약

CLI의 기존 `xgen tools describe --name collection tool`은 정규화된 전체 후보·보존 계약을 반환한다. 모델이 보는 view는 다음으로 확인한다.

```bash
xgen tools describe --name inventory readAsset --model-view
xgen tools describe --name inventory readAsset --model-view --parameter-offset 32
```

agent loop의 `xgen.tools/describe@2.0.0` 입력은 `{collection, tool, parameterOffset?}`다. 기본 offset은 0이다. collection snapshot과 입력 페이지는 기존 material digest·권한·Receipt에 고정한다. `parameter_page.next_offset`이 있으면 그 값으로 다음 페이지를 요청한다. 페이지당 최대 32개이며 bytes 한도로 더 적을 수 있다. 동일 snapshot·tool에서는 입력 순서가 고정되고 마지막 페이지는 next_offset=null이다.

결과는 `contract_kind=discovery_view`, `view_version=2`, `complete=false`, 효과 미분류·실행 비활성 상태다. `full_tool_digest`는 생략 전 전체 normalized tool의 digest다. 표시용 문자열은 `{text, omitted}`로 생략 여부를 표시한다. 입력 이름이 생략되면 그 표시 문자열을 완전한 이름으로 사용하면 안 된다.

작은 schema는 `schema`와 `complete=true`로 제공한다. 큰 schema는 digest·bytes·일부 제약·enum 개수를 제공하고 `complete=false`다. HTTP 지원 후보의 `schema_origin=http_original`과 미지원 후보의 `discovery_normalized`를 구분한다. 후자의 type/enum은 원본 실행 schema가 아니다. 큰 enum의 모든 값이나 중첩 schema의 세부 조회는 아직 제공하지 않는다. 이것이 모델의 유효한 입력 선택을 어렵게 할 수 있으며 후속 평가가 필요하다.

HTTP 지원 여부·거절 코드와 전체 `http_read.contract_digest`를 제공하되, 실행용 `contract`는 모델에 넣지 않는다. 응답 schema는 digest로만 표시하며 일부 status만 표시할 수 있어 개수와 생략 상태를 함께 제공한다. worker view는 ASCII canonical JSON 기준 48 KiB 이내이고 Rust의 요청 추가 후 관찰 한도는 기존 64 KiB다. 한도를 넘으면 성공/조용한 잘림 대신 `tool_discovery_output_limit` 실패를 기록한다.

## 실행 검증

`xgen.http/read@1.0.0`은 모델이 전달한 전체 계약 digest를 호스트가 다시 계산한 값과 비교한다. 별도 offline `execution_contract` 경로에서 전체 원본 HTTP 계약을 가져오고, 호출 전에 모든 파라미터 schema·인증 조건을 검증한다. 실제 응답은 모든 원본 성공 schema 조건으로 검증한다. view가 생략한 enum·required·response 제약도 적용한다.

전체 HTTP 계약은 8 MiB, worker 응답은 기존 16 MiB·120초로 제한한다. 제한을 넘거나 지원하지 않는 schema는 호출을 거절한다. JSON body·URL·정규화 입력·인증·redirect/proxy/retry 조건은 [HTTP 실행 계약](http-read-tools.md)을 따른다. 원본 계약이나 인증값을 모델에 자동 전송하지 않는다.

## 버전 이행

search·HTTP read는 1.0.0이고 describe만 2.0.0이다. 정의·adapter binding·승인·materializer·verifier가 작업별 버전을 확인한다. execution profile에는 정의와 binding이 포함되므로 이전 discovery profile로 진행 중인 Run은 구성 불일치로 재개를 거절한다. 새 Run으로 시작해야 한다. 완료된 Run의 저장 결과 재생은 기존 경로를 사용하며 추가 API 호출이 없다. collection 저장 형식·source digest·작은 기존 HTTP 계약 digest는 유지한다.

결과는 [검증 계획](bounded-tool-describe-plan.md)과 [별도 결과 기록](bounded-tool-describe-2026-10-06.md)을 따른다. 전달 문제 해결을 검색 품질이나 실제 LLM 선택 품질 개선으로 해석하지 않는다.
