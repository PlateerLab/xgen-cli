"""Generate synthetic, matched source-size fixtures; never production code."""
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent
EVIDENCE = json.loads((ROOT.parent / 'response-contracts-v2/cases.json').read_text())[0]['response_contract'].replace('소스 코드 블록 외에 ', '')


def build():
    cases = []
    for family in ('python-pair', 'node-pair'):
        for size, count in [('small', 4), ('large', 2048)]:
            numbers = ', '.join(map(str, range(count)))
            if family == 'python-pair':
                sources = ['bounds.py', 'chunks.py']
                prefix = f'SOURCE_IDS = [{numbers}]\n\n'
                files = {'bounds.py': prefix + 'def clamp_values(items, low, high):\n    return [min(low, max(high, value)) for value in items]\n',
                         'chunks.py': prefix + 'def chunks(items, width):\n    return [items[i:i+width] for i in range(0, len(items)-width+1, width)]\n',
                         'test_visible.py': '''import unittest
from bounds import clamp_values
from chunks import chunks
class Visible(unittest.TestCase):
    def test_bounds(self):
        self.assertEqual(clamp_values([-3,0,5,10], 0, 5), [0,0,5,5])
    def test_chunks(self):
        self.assertEqual(chunks([1,2,3], 2), [[1,2],[3]])
'''}
                hidden = f'''import unittest
import bounds, chunks
class Hidden(unittest.TestCase):
    def test_contract(self):
        data = [-2,3,9]
        self.assertEqual(bounds.clamp_values(data, 3, 3), [3,3,3])
        self.assertEqual(bounds.clamp_values([], 0, 1), [])
        self.assertEqual(data, [-2,3,9])
        self.assertEqual(chunks.chunks([], 4), [])
        self.assertEqual(chunks.chunks(['가','나'], 8), [['가','나']])
        self.assertEqual(chunks.chunks([1,2,3,4], 2), [[1,2],[3,4]])
        self.assertEqual(bounds.SOURCE_IDS, list(range({count})))
        self.assertEqual(chunks.SOURCE_IDS, list(range({count})))
'''
                commands = [['python3','-m','unittest','-v'], ['python3','-m','compileall','-q','.']]
                contract = 'bounds.py의 clamp_values(items,low,high)는 숫자 배열의 각 값을 닫힌 구간 [low,high] 안으로 제한한 새 리스트를 반환한다. low<=high이며 빈 입력과 low==high를 허용하고 원본을 수정하지 않는다. chunks.py의 chunks(items,width)는 임의 값 리스트를 양의 정수 width 크기로 순서대로 분할하고 마지막 짧은 묶음도 보존한다. 빈 입력은 []이며 원본을 수정하지 않는다. 두 모듈의 기존 SOURCE_IDS 전체 내용·이름·순서는 보존한다.'
                lang, hidden_path = 'python', 'test_hidden.py'
            else:
                sources = ['flatten.mjs', 'repeat.mjs']
                prefix = f'export const SOURCE_IDS = [{numbers}];\n\n'
                files = {'flatten.mjs': prefix + 'export function flattenOne(rows) {\n  return rows.flat(2);\n}\n',
                         'repeat.mjs': prefix + 'export function repeatValues(items, count) {\n  return Array(count).fill(items).flat();\n}\n',
                         'package.json': '{"type":"module","private":true}\n',
                         'visible.test.mjs': '''import test from 'node:test';
import assert from 'node:assert/strict';
import {flattenOne} from './flatten.mjs';
import {repeatValues} from './repeat.mjs';
test('one level only', () => assert.deepEqual(flattenOne([[1,[2]],[3]]),[1,[2],3]));
test('repeat each item', () => assert.deepEqual(repeatValues([1,2],2),[1,1,2,2]));
'''}
                hidden = f'''import test from 'node:test';
import assert from 'node:assert/strict';
import * as flatten from './flatten.mjs';
import * as repeat from './repeat.mjs';
test('empty, identity, zeros and metadata preserved', () => {{
  const cell = [1], obj = {{}};
  const rows = [[cell,obj],[],[null,false]];
  const result = flatten.flattenOne(rows);
  assert.deepEqual(result,[cell,obj,null,false]); assert.equal(result[0],cell);
  assert.deepEqual(rows,[[cell,obj],[],[null,false]]);
  assert.deepEqual(flatten.flattenOne([]),[]);
  assert.deepEqual(repeat.repeatValues([cell,obj],2),[cell,cell,obj,obj]);
  assert.equal(repeat.repeatValues([cell],2)[0],cell);
  assert.deepEqual(repeat.repeatValues([1],0),[]);
  assert.deepEqual(repeat.repeatValues([],3),[]);
  const expected = Array.from({{length:{count}}}, (_,i)=>i);
  assert.deepEqual(flatten.SOURCE_IDS,expected); assert.deepEqual(repeat.SOURCE_IDS,expected);
}});
'''
                commands = [['node','--test','visible.test.mjs'], ['node','--check','flatten.mjs']]
                contract = 'flatten.mjs의 flattenOne(rows)는 배열들의 배열을 받아 각 row의 항목을 한 단계만 합친 새 배열을 반환한다. row의 항목이 배열이나 객체여도 더 펼치지 않고 identity와 순서를 보존한다. 빈 row·빈 입력을 허용하고 원본을 수정하지 않는다. repeat.mjs의 repeatValues(items,count)는 각 항목을 연속 count번 반복한 새 배열을 반환한다. count는 0 이상의 정수이고 빈 입력·count=0은 []다. 중첩 배열·객체·null·false도 펼치지 않고 identity를 보존하며 원본을 수정하지 않는다. 두 모듈의 기존 SOURCE_IDS 전체 내용·이름·순서는 보존한다.'
                lang, hidden_path = 'node', 'hidden.test.mjs'
            for response in ('quote', 'summary'):
                cases.append({'id':f'{family}-{size}-{response}', 'family':family, 'size':size,
                              'split':'design' if lang=='python' else 'held_out', 'response_mode':response,
                              'language':lang, 'mode':'run-resume', 'source':sources[0], 'sources':sources,
                              'contract':contract, 'response_contract':EVIDENCE, 'files':files,
                              'commands':commands, 'hidden_path':hidden_path, 'hidden_tests':hidden})
    return cases


if __name__ == '__main__':
    (ROOT / 'cases.json').write_text(json.dumps(build(),ensure_ascii=False,indent=2)+'\n')
