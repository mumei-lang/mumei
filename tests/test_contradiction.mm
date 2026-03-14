// test_contradiction.mm
// Pos の制約 (v > 0) と requires (n < 0) が矛盾する
// unsat_core には track_refined_type_n::Pos と track_requires が含まれるはず
type Pos = i64 where v > 0;

atom impossible(n: Pos): i64
  requires: n < 0;
  ensures: true;
  body: n;
