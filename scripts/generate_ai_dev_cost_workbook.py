#!/usr/bin/env python3
"""Generate a dependency-free Excel template for tracking AI development costs."""

from __future__ import annotations

import argparse
import datetime as dt
import zipfile
from pathlib import Path
from xml.sax.saxutils import escape


STAGES = [
    "需求分析与架构设计",
    "Rust 核心逻辑与数据管线",
    "LLM/API 集成与端到端编排",
    "测试调试与质量验证",
    "UI 开发与可视化",
    "部署、Rain Classroom 集成与优化",
]

# These are deliberately marked as estimates: Codex subscription usage is not
# exposed as a public per-token bill. Community reports put a Pro 5x weekly
# allowance anywhere from roughly 0.5B to 1.2B local tokens. We use a midpoint
# of about 0.85B: 649M project tokens is ~76% of one weekly allowance. The USD
# column converts that share using the user's $100/month reference, i.e. about
# $25 per week, not $100 per week.
ESTIMATES = [
    (12, 51_958_567, 1.52, "gpt-5.6-sol", "Codex CLI/TUI", "8/27–8/28；按抽样聊天记录归类"),
    (38, 155_875_701, 4.56, "gpt-5.6-sol", "Codex CLI/TUI", "8/28–8/30；Rust 核心、检索、窗口与数据结构"),
    (24, 116_906_776, 3.42, "gpt-5.6-sol；GLM-5（外部测试）", "Codex CLI/TUI", "8/30–9/3；API canary、编排、转写恢复与标注"),
    (32, 116_906_776, 3.42, "gpt-5.6-sol；gpt-6-astra", "Codex CLI/TUI", "9/3–9/6；验证、修复、回归与发布检查"),
    (26, 103_917_134, 3.04, "gpt-5.6-sol", "Codex CLI/TUI + browser 工具", "9/4–9/5、9/9；阅读视图、滑块、幻灯片联动"),
    (34, 103_917_134, 3.04, "gpt-6-astra；gpt-5.6-sol", "Codex CLI/TUI + Playwright", "9/6–9/13；打包、Rain Classroom、OCR、预算与性能"),
]

HEADERS = [
    "阶段",
    "人时（小时）",
    "总 Token 数",
    "总花费（USD）",
    "汇率（USD→CNY）",
    "总花费（CNY）",
    "模型名称",
    "AI 开发工具",
    "备注",
]


def inline_cell(ref: str, value: str, style: int) -> str:
    return (
        f'<c r="{ref}" s="{style}" t="inlineStr">'
        f"<is><t>{escape(value)}</t></is></c>"
    )


def blank_cell(ref: str, style: int) -> str:
    return f'<c r="{ref}" s="{style}"/>'


def number_cell(ref: str, value: int | float, style: int) -> str:
    return f'<c r="{ref}" s="{style}"><v>{value}</v></c>'


def formula_cell(ref: str, formula: str, style: int) -> str:
    return f'<c r="{ref}" s="{style}"><f>{escape(formula)}</f></c>'


def worksheet_xml() -> str:
    rows: list[str] = [
        '<row r="1" ht="30" customHeight="1">' + inline_cell("A1", "AI 开发开销明细表（聊天记录估算）", 1) + "</row>",
        '<row r="2" ht="48" customHeight="1">' + inline_cell("A2", "黄色单元格为可填写区；Token/人时来自聊天记录抽样。社区实测显示 Pro 5x 周额度约 0.5–1.2B Token（波动很大）；本表取约 0.85B 的中位情形，项目 649M Token ≈ 76% 周额度。按 $100/月≈$25/周折算，费用约 $19，合理区间约 $14–$33；不是精确账单。", 2) + "</row>",
        '<row r="3" ht="22" customHeight="1"><c r="A3" s="11"/></row>',
    ]
    header_cells = "".join(inline_cell(f"{chr(65 + index)}4", header, 3) for index, header in enumerate(HEADERS))
    rows.append(f'<row r="4" ht="32" customHeight="1">{header_cells}</row>')

    for row_number in range(5, 25):
        index = row_number - 5
        cells = [inline_cell(f"A{row_number}", STAGES[index] if index < len(STAGES) else "", 4)]
        if index < len(ESTIMATES):
            hours, tokens, usd, model, tool, note = ESTIMATES[index]
            cells.extend([number_cell(f"B{row_number}", hours, 6), number_cell(f"C{row_number}", tokens, 7), number_cell(f"D{row_number}", usd, 8), number_cell(f"E{row_number}", 7.2, 10)])
            cells.append(formula_cell(f"F{row_number}", f'IF(OR(D{row_number}="",E{row_number}=""),"",D{row_number}*E{row_number})', 13))
            cells.extend([inline_cell(f"G{row_number}", model, 5), inline_cell(f"H{row_number}", tool, 5), inline_cell(f"I{row_number}", note, 5)])
        else:
            cells.extend([blank_cell(f"{col}{row_number}", style) for col, style in (("B", 6), ("C", 7), ("D", 8), ("E", 10))])
            cells.append(formula_cell(f"F{row_number}", f'IF(OR(D{row_number}="",E{row_number}=""),"",D{row_number}*E{row_number})', 13))
            cells.extend([blank_cell(f"{col}{row_number}", 5) for col in ("G", "H", "I")])
        rows.append(f'<row r="{row_number}" ht="30" customHeight="1">{"".join(cells)}</row>')

    total_cells = [inline_cell("A25", "合计", 14)]
    for column, style in (("B", 15), ("C", 15), ("D", 16)):
        total_cells.append(formula_cell(f"{column}25", f"SUM({column}5:{column}24)", style))
    total_cells.append(blank_cell("E25", 14))
    total_cells.append(formula_cell("F25", "SUM(F5:F24)", 17))
    total_cells.extend([blank_cell(f"{col}25", 14) for col in ("G", "H", "I")])
    rows.append(f'<row r="25" ht="26" customHeight="1">{"".join(total_cells)}</row>')

    return f'''<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"
 xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <dimension ref="A1:I25"/>
  <sheetViews>
    <sheetView workbookViewId="0" showGridLines="0">
      <pane ySplit="4" topLeftCell="A5" activePane="bottomLeft" state="frozen"/>
      <selection pane="bottomLeft" activeCell="A5" sqref="A5"/>
    </sheetView>
  </sheetViews>
  <sheetFormatPr defaultRowHeight="18"/>
  <cols>
    <col min="1" max="1" width="30" customWidth="1"/>
    <col min="2" max="2" width="14" customWidth="1"/>
    <col min="3" max="3" width="18" customWidth="1"/>
    <col min="4" max="6" width="18" customWidth="1"/>
    <col min="7" max="8" width="28" customWidth="1"/>
    <col min="9" max="9" width="42" customWidth="1"/>
  </cols>
  <sheetData>{''.join(rows)}</sheetData>
  <mergeCells count="2"><mergeCell ref="A1:I1"/><mergeCell ref="A2:I2"/></mergeCells>
  <autoFilter ref="A4:I24"/>
  <tableParts count="1"><tablePart r:id="rId1"/></tableParts>
  <pageMargins left="0.3" right="0.3" top="0.5" bottom="0.5" header="0.2" footer="0.2"/>
  <pageSetup orientation="landscape" fitToWidth="1" fitToHeight="0"/>
</worksheet>'''


def styles_xml() -> str:
    return '''<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <numFmts count="4">
    <numFmt numFmtId="164" formatCode="0.00"/>
    <numFmt numFmtId="165" formatCode="$#,##0.00;[Red]-$#,##0.00"/>
    <numFmt numFmtId="166" formatCode="0.0000"/>
    <numFmt numFmtId="167" formatCode="¥#,##0.00;[Red]-¥#,##0.00"/>
  </numFmts>
  <fonts count="4">
    <font><sz val="11"/><name val="Aptos"/><family val="2"/><scheme val="minor"/></font>
    <font><b/><color rgb="FFFFFFFF"/><sz val="18"/><name val="Aptos Display"/><family val="2"/><scheme val="major"/></font>
    <font><b/><color rgb="FFFFFFFF"/><sz val="11"/><name val="Aptos"/><family val="2"/><scheme val="minor"/></font>
    <font><b/><color rgb="FF1F1F1F"/><sz val="11"/><name val="Aptos"/><family val="2"/><scheme val="minor"/></font>
  </fonts>
  <fills count="7">
    <fill><patternFill patternType="none"/></fill>
    <fill><patternFill patternType="gray125"/></fill>
    <fill><patternFill patternType="solid"><fgColor rgb="FF1F4E78"/><bgColor indexed="64"/></patternFill></fill>
    <fill><patternFill patternType="solid"><fgColor rgb="FF2F75B5"/><bgColor indexed="64"/></patternFill></fill>
    <fill><patternFill patternType="solid"><fgColor rgb="FFFFF2CC"/><bgColor indexed="64"/></patternFill></fill>
    <fill><patternFill patternType="solid"><fgColor rgb="FFD9EAF7"/><bgColor indexed="64"/></patternFill></fill>
    <fill><patternFill patternType="solid"><fgColor rgb="FFD9EAD3"/><bgColor indexed="64"/></patternFill></fill>
  </fills>
  <borders count="2">
    <border><left/><right/><top/><bottom/><diagonal/></border>
    <border><left style="thin"><color rgb="FFD9E1F2"/></left><right style="thin"><color rgb="FFD9E1F2"/></right><top style="thin"><color rgb="FFD9E1F2"/></top><bottom style="thin"><color rgb="FFD9E1F2"/></bottom><diagonal/></border>
  </borders>
  <cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>
  <cellXfs count="18">
    <xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/>
    <xf numFmtId="0" fontId="1" fillId="2" borderId="0" xfId="0" applyFont="1" applyFill="1" applyAlignment="1"><alignment horizontal="left" vertical="center"/></xf>
    <xf numFmtId="0" fontId="0" fillId="5" borderId="0" xfId="0" applyFill="1" applyAlignment="1"><alignment horizontal="left" vertical="center" wrapText="1"/></xf>
    <xf numFmtId="0" fontId="2" fillId="3" borderId="1" xfId="0" applyFont="1" applyFill="1" applyBorder="1" applyAlignment="1"><alignment horizontal="center" vertical="center" wrapText="1"/></xf>
    <xf numFmtId="0" fontId="0" fillId="4" borderId="1" xfId="0" applyFill="1" applyBorder="1" applyAlignment="1"><alignment horizontal="left" vertical="center" wrapText="1"/></xf>
    <xf numFmtId="0" fontId="0" fillId="4" borderId="1" xfId="0" applyFill="1" applyBorder="1" applyAlignment="1"><alignment horizontal="left" vertical="top" wrapText="1"/></xf>
    <xf numFmtId="164" fontId="0" fillId="4" borderId="1" xfId="0" applyNumberFormat="1" applyFill="1" applyBorder="1" applyAlignment="1"><alignment horizontal="right" vertical="center"/></xf>
    <xf numFmtId="3" fontId="0" fillId="4" borderId="1" xfId="0" applyNumberFormat="1" applyFill="1" applyBorder="1" applyAlignment="1"><alignment horizontal="right" vertical="center"/></xf>
    <xf numFmtId="165" fontId="0" fillId="4" borderId="1" xfId="0" applyNumberFormat="1" applyFill="1" applyBorder="1" applyAlignment="1"><alignment horizontal="right" vertical="center"/></xf>
    <xf numFmtId="0" fontId="3" fillId="5" borderId="1" xfId="0" applyFont="1" applyFill="1" applyBorder="1" applyAlignment="1"><alignment horizontal="left" vertical="center" wrapText="1"/></xf>
    <xf numFmtId="166" fontId="3" fillId="4" borderId="1" xfId="0" applyNumberFormat="1" applyFont="1" applyFill="1" applyBorder="1" applyAlignment="1"><alignment horizontal="right" vertical="center"/></xf>
    <xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0" applyAlignment="1"><alignment horizontal="left" vertical="center"/></xf>
    <xf numFmtId="166" fontId="0" fillId="0" borderId="1" xfId="0" applyNumberFormat="1" applyBorder="1" applyAlignment="1"><alignment horizontal="right" vertical="center"/></xf>
    <xf numFmtId="167" fontId="0" fillId="0" borderId="1" xfId="0" applyNumberFormat="1" applyBorder="1" applyAlignment="1"><alignment horizontal="right" vertical="center"/></xf>
    <xf numFmtId="0" fontId="3" fillId="6" borderId="1" xfId="0" applyFont="1" applyFill="1" applyBorder="1" applyAlignment="1"><alignment horizontal="left" vertical="center"/></xf>
    <xf numFmtId="3" fontId="3" fillId="6" borderId="1" xfId="0" applyNumberFormat="1" applyFont="1" applyFill="1" applyBorder="1" applyAlignment="1"><alignment horizontal="right" vertical="center"/></xf>
    <xf numFmtId="165" fontId="3" fillId="6" borderId="1" xfId="0" applyNumberFormat="1" applyFont="1" applyFill="1" applyBorder="1" applyAlignment="1"><alignment horizontal="right" vertical="center"/></xf>
    <xf numFmtId="167" fontId="3" fillId="6" borderId="1" xfId="0" applyNumberFormat="1" applyFont="1" applyFill="1" applyBorder="1" applyAlignment="1"><alignment horizontal="right" vertical="center"/></xf>
  </cellXfs>
  <cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles>
</styleSheet>'''


def table_xml() -> str:
    columns = "".join(
        f'<tableColumn id="{index}" name="{escape(header)}"/>'
        for index, header in enumerate(HEADERS, start=1)
    )
    return f'''<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<table xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" id="1" name="AIDevCostTable" displayName="AIDevCostTable" ref="A4:I24" totalsRowShown="0">
  <autoFilter ref="A4:I24"/>
  <tableColumns count="{len(HEADERS)}">{columns}</tableColumns>
  <tableStyleInfo name="TableStyleMedium2" showFirstColumn="0" showLastColumn="0" showRowStripes="1" showColumnStripes="0"/>
</table>'''


def create_workbook(output: Path) -> None:
    output.parent.mkdir(parents=True, exist_ok=True)
    created = dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")
    files = {
        "[Content_Types].xml": '''<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
  <Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>
  <Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/>
  <Override PartName="/xl/tables/table1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.table+xml"/>
  <Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/>
  <Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/>
</Types>''',
        "_rels/.rels": '''<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
  <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/>
  <Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties" Target="docProps/app.xml"/>
</Relationships>''',
        "docProps/app.xml": '''<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties" xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes">
  <Application>Microsoft Excel Compatible Template Generator</Application>
  <DocSecurity>0</DocSecurity><ScaleCrop>false</ScaleCrop>
  <HeadingPairs><vt:vector size="2" baseType="variant"><vt:variant><vt:lpstr>Worksheets</vt:lpstr></vt:variant><vt:variant><vt:i4>1</vt:i4></vt:variant></vt:vector></HeadingPairs>
  <TitlesOfParts><vt:vector size="1" baseType="lpstr"><vt:lpstr>AI 开发开销</vt:lpstr></vt:vector></TitlesOfParts>
  <Company></Company><LinksUpToDate>false</LinksUpToDate><SharedDoc>false</SharedDoc><HyperlinksChanged>false</HyperlinksChanged><AppVersion>16.0300</AppVersion>
</Properties>''',
        "docProps/core.xml": f'''<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:dcmitype="http://purl.org/dc/dcmitype/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
  <dc:creator>Codex</dc:creator><cp:lastModifiedBy>Codex</cp:lastModifiedBy><dc:title>AI 开发开销明细表</dc:title>
  <dcterms:created xsi:type="dcterms:W3CDTF">{created}</dcterms:created><dcterms:modified xsi:type="dcterms:W3CDTF">{created}</dcterms:modified>
</cp:coreProperties>''',
        "xl/workbook.xml": '''<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <bookViews><workbookView xWindow="0" yWindow="0" windowWidth="24000" windowHeight="12000"/></bookViews>
  <sheets><sheet name="AI 开发开销" sheetId="1" state="visible" r:id="rId1"/></sheets>
  <calcPr calcId="191029" calcMode="auto" fullCalcOnLoad="1" forceFullCalc="1"/>
</workbook>''',
        "xl/_rels/workbook.xml.rels": '''<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
  <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>
</Relationships>''',
        "xl/worksheets/sheet1.xml": worksheet_xml(),
        "xl/worksheets/_rels/sheet1.xml.rels": '''<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/table" Target="../tables/table1.xml"/>
</Relationships>''',
        "xl/styles.xml": styles_xml(),
        "xl/tables/table1.xml": table_xml(),
    }
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for name, content in files.items():
            archive.writestr(name, content.encode("utf-8"))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "output",
        nargs="?",
        type=Path,
        default=Path("docs/AI开发开销明细表.xlsx"),
        help="Output .xlsx path (default: docs/AI开发开销明细表.xlsx)",
    )
    args = parser.parse_args()
    create_workbook(args.output)
    print(args.output.resolve())


if __name__ == "__main__":
    main()
