use std::error::Error;

use beyond_slides::{PassageProjectionError, project_passage_boundaries};

#[test]
fn exact_passages_reuse_authoritative_text_ranges() -> Result<(), Box<dyn Error>> {
    let source = "所有权负责资源管理。借用让函数临时访问数据。";
    let projection =
        project_passage_boundaries(source, ["所有权负责资源管理。", "借用让函数临时访问数据。"])?;

    assert!(projection.is_exact());
    assert_eq!(projection.changed_characters(), 0);
    assert_eq!(projected_text(source, &projection), source);
    assert_eq!(
        projected_passages(source, &projection),
        vec!["所有权负责资源管理。", "借用让函数临时访问数据。"]
    );
    Ok(())
}

#[test]
fn a_missing_chinese_character_is_recovered_from_the_source() -> Result<(), Box<dyn Error>> {
    let source = "这里先回顾模式匹配的基本形式。你说我想把它 match 出来，然后再处理内部的数据。";
    let projection = project_passage_boundaries(
        source,
        [
            "这里先回顾模式匹配的基本形式。",
            "说我想把它 match 出来，然后再处理内部的数据。",
        ],
    )?;

    assert!(!projection.is_exact());
    assert_eq!(projection.changed_characters(), 1);
    assert_eq!(projected_text(source, &projection), source);
    assert_eq!(
        projected_passages(source, &projection),
        vec![
            "这里先回顾模式匹配的基本形式。",
            "你说我想把它 match 出来，然后再处理内部的数据。"
        ]
    );
    Ok(())
}

#[test]
fn a_small_code_correction_is_discarded_after_boundary_projection() -> Result<(), Box<dyn Error>> {
    let source = "约束可以写成 Iterator<Item: Clone>，这里先关注语法以及它在泛型函数中的作用。接下来讨论编译器如何检查这个约束，并给出一个完整的实现示例。";
    let projection = project_passage_boundaries(
        source,
        [
            "约束可以写成 Iterator<Item = Clone>，这里先关注语法以及它在泛型函数中的作用。",
            "接下来讨论编译器如何检查这个约束，并给出一个完整的实现示例。",
        ],
    )?;

    assert_eq!(projection.changed_characters(), 3);
    assert!(projection.error_ratio() < 0.05);
    assert_eq!(projected_text(source, &projection), source);
    assert_eq!(
        projected_passages(source, &projection)[0],
        "约束可以写成 Iterator<Item: Clone>，这里先关注语法以及它在泛型函数中的作用。"
    );
    Ok(())
}

#[test]
fn a_character_deleted_at_a_boundary_is_assigned_deterministically() -> Result<(), Box<dyn Error>> {
    let source = "第一部分解释所有权。然后讨论借用规则以及它为什么安全。";
    let projection = project_passage_boundaries(
        source,
        ["第一部分解释所有权。", "后讨论借用规则以及它为什么安全。"],
    )?;

    assert_eq!(projected_text(source, &projection), source);
    assert_eq!(
        projected_passages(source, &projection),
        vec!["第一部分解释所有权。", "然后讨论借用规则以及它为什么安全。"]
    );
    Ok(())
}

#[test]
fn repeated_text_still_produces_complete_contiguous_ranges() -> Result<(), Box<dyn Error>> {
    let source = "这个类型可以复制，复制以后还可以复制，最后再解释复制的限制。";
    let projection = project_passage_boundaries(
        source,
        [
            "这个类型可以复制，",
            "复制以后还可以复制，",
            "最后再解释复制的限制。",
        ],
    )?;

    assert_eq!(projected_text(source, &projection), source);
    assert!(
        projection
            .byte_ranges()
            .windows(2)
            .all(|ranges| ranges[0].end == ranges[1].start)
    );
    Ok(())
}

#[test]
fn unicode_ranges_are_valid_utf8_boundaries() -> Result<(), Box<dyn Error>> {
    let source = "Rust 🦀 很安全，也能在编译阶段发现许多内存错误。所有权避免悬垂引用，并明确每一份资源由谁释放。";
    let projection = project_passage_boundaries(
        source,
        [
            "Rust 很安全，也能在编译阶段发现许多内存错误。",
            "所有权避免悬垂引用，并明确每一份资源由谁释放。",
        ],
    )?;

    assert_eq!(projected_text(source, &projection), source);
    assert_eq!(
        projected_passages(source, &projection),
        vec![
            "Rust 🦀 很安全，也能在编译阶段发现许多内存错误。",
            "所有权避免悬垂引用，并明确每一份资源由谁释放。"
        ]
    );
    Ok(())
}

#[test]
fn a_difference_at_the_five_percent_limit_is_rejected() {
    let source = "abcdefghijklmnopqrst";
    let error = project_passage_boundaries(source, ["abcdefghij", "klmnopqrsX"])
        .expect_err("one changed character out of twenty reaches the strict limit");

    assert_eq!(
        error,
        PassageProjectionError::DifferenceTooLarge {
            changed_characters: 1,
            compared_characters: 20,
        }
    );
}

#[test]
fn proposed_passages_cannot_collapse_to_an_empty_source_range() {
    let source = "abcdefghijklmnopqrstuvwxyz0123456789";
    let error =
        project_passage_boundaries(source, ["a", "X", "bcdefghijklmnopqrstuvwxyz0123456789"])
            .expect_err("a passage containing only inserted text has no source-backed content");

    assert_eq!(
        error,
        PassageProjectionError::EmptyProjectedPassage { index: 1 }
    );
}

fn projected_text(source: &str, projection: &beyond_slides::PassageProjection) -> String {
    projected_passages(source, projection).concat()
}

fn projected_passages<'a>(
    source: &'a str,
    projection: &beyond_slides::PassageProjection,
) -> Vec<&'a str> {
    projection
        .byte_ranges()
        .iter()
        .map(|range| &source[range.clone()])
        .collect()
}
