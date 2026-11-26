; ModuleID = 'translation_validation_transfer.ll'
source_filename = "translation_validation_transfer.ll"
target datalayout = "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-f80:128-n8:16:32:64-S128"
target triple = "x86_64-pc-linux-gnu"

define { i64, i64, i64, i64 } @transfer(i64 %0, i64 %1, i64 %2, i64 %3, i64 %4, i64 %5) {
entry:
  %cmp_tmp = icmp sle i64 %5, 0
  %bool_tmp = zext i1 %cmp_tmp to i64
  %if_cond = icmp ne i64 %bool_tmp, 0
  br i1 %if_cond, label %then, label %else

then:                                             ; preds = %entry
  %struct_src = insertvalue { i64, i64, i64, i64 } { i64 0, i64 undef, i64 undef, i64 undef }, i64 %0, 1
  %struct_dst = insertvalue { i64, i64, i64, i64 } %struct_src, i64 %1, 2
  %struct_house = insertvalue { i64, i64, i64, i64 } %struct_dst, i64 %2, 3
  br label %merge

else:                                             ; preds = %entry
  %if_cond1 = icmp ne i64 %3, 0
  br i1 %if_cond1, label %then2, label %else3

merge:                                            ; preds = %merge13, %then
  %if_result39 = phi { i64, i64, i64, i64 } [ %struct_house, %then ], [ %if_result38, %merge13 ]
  ret { i64, i64, i64, i64 } %if_result39

then2:                                            ; preds = %else
  br label %merge4

else3:                                            ; preds = %else
  br label %merge4

merge4:                                           ; preds = %else3, %then2
  %if_result = phi i64 [ 0, %then2 ], [ 1, %else3 ]
  %logic_lhs = icmp ne i64 %if_result, 0
  br i1 %logic_lhs, label %logic.merge, label %logic.rhs

logic.rhs:                                        ; preds = %merge4
  %if_cond5 = icmp ne i64 %4, 0
  br i1 %if_cond5, label %then6, label %else7

logic.merge:                                      ; preds = %merge8, %merge4
  %logic_result = phi i1 [ true, %merge4 ], [ %logic_rhs, %merge8 ]
  %logic_ext = zext i1 %logic_result to i64
  %if_cond10 = icmp ne i64 %logic_ext, 0
  br i1 %if_cond10, label %then11, label %else12

then6:                                            ; preds = %logic.rhs
  br label %merge8

else7:                                            ; preds = %logic.rhs
  br label %merge8

merge8:                                           ; preds = %else7, %then6
  %if_result9 = phi i64 [ 0, %then6 ], [ 1, %else7 ]
  %logic_rhs = icmp ne i64 %if_result9, 0
  br label %logic.merge

then11:                                           ; preds = %logic.merge
  %struct_src14 = insertvalue { i64, i64, i64, i64 } { i64 0, i64 undef, i64 undef, i64 undef }, i64 %0, 1
  %struct_dst15 = insertvalue { i64, i64, i64, i64 } %struct_src14, i64 %1, 2
  %struct_house16 = insertvalue { i64, i64, i64, i64 } %struct_dst15, i64 %2, 3
  br label %merge13

else12:                                           ; preds = %logic.merge
  %add_tmp = add i64 %5, 25
  %cmp_tmp17 = icmp sge i64 %0, %add_tmp
  %bool_tmp18 = zext i1 %cmp_tmp17 to i64
  %if_cond19 = icmp ne i64 %bool_tmp18, 0
  br i1 %if_cond19, label %then20, label %else21

merge13:                                          ; preds = %merge27, %then11
  %if_result38 = phi { i64, i64, i64, i64 } [ %struct_house16, %then11 ], [ %if_result37, %merge27 ]
  br label %merge

then20:                                           ; preds = %else12
  br label %merge22

else21:                                           ; preds = %else12
  br label %merge22

merge22:                                          ; preds = %else21, %then20
  %if_result23 = phi i64 [ 0, %then20 ], [ 1, %else21 ]
  %if_cond24 = icmp ne i64 %if_result23, 0
  br i1 %if_cond24, label %then25, label %else26

then25:                                           ; preds = %merge22
  %struct_src28 = insertvalue { i64, i64, i64, i64 } { i64 0, i64 undef, i64 undef, i64 undef }, i64 %0, 1
  %struct_dst29 = insertvalue { i64, i64, i64, i64 } %struct_src28, i64 %1, 2
  %struct_house30 = insertvalue { i64, i64, i64, i64 } %struct_dst29, i64 %2, 3
  br label %merge27

else26:                                           ; preds = %merge22
  %sub_tmp = sub i64 %0, %5
  %sub_tmp31 = sub i64 %sub_tmp, 25
  %add_tmp32 = add i64 %1, %5
  %add_tmp33 = add i64 %2, 25
  %struct_src34 = insertvalue { i64, i64, i64, i64 } { i64 1, i64 undef, i64 undef, i64 undef }, i64 %sub_tmp31, 1
  %struct_dst35 = insertvalue { i64, i64, i64, i64 } %struct_src34, i64 %add_tmp32, 2
  %struct_house36 = insertvalue { i64, i64, i64, i64 } %struct_dst35, i64 %add_tmp33, 3
  br label %merge27

merge27:                                          ; preds = %else26, %then25
  %if_result37 = phi { i64, i64, i64, i64 } [ %struct_house30, %then25 ], [ %struct_house36, %else26 ]
  br label %merge13
}

define i64 @__mumei_user_main() {
entry:
  ret i64 0
}

define i32 @main(i32 %0, ptr %1) {
entry:
  %result = call i64 @__mumei_user_main()
  %exit_code = trunc i64 %result to i32
  ret i32 %exit_code
}
