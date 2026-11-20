; Unshipped analyzer release
; https://github.com/dotnet/roslyn-analyzers/blob/main/src/Microsoft.CodeAnalysis.Analyzers/ReleaseTrackingAnalyzers.Help.md

### New Rules

| Rule ID  | Category              | Severity | Notes                                                         |
| -------- | --------------------- | -------- | ------------------------------------------------------------- |
| PILL0101 | PillSystemDeclaration | Error    | An ECS system must be static                                  |
| PILL0102 | PillSystemDeclaration | Error    | An ECS system has an unsupported parameter list               |
| PILL0103 | PillSystemDeclaration | Error    | An ECS system must return void                                |
| PILL0104 | PillSystemDeclaration | Error    | A [System] condition must be read-only and return bool        |
| PILL0105 | PillSystemDeclaration | Error    | A [System] RunIf condition names no method                    |
| PILL0201 | PillFrameScope        | Error    | An ECS system may not be async                                |
| PILL0202 | PillFrameScope        | Error    | A parallel row body may not touch engine state                |
| PILL0301 | PillStaticState       | Error    | Mutable static state in a type that declares ECS systems      |
| PILL0302 | PillStaticState       | Error    | A static event prevents the project assembly from unloading   |
| PILL0303 | PillStaticState       | Error    | Background work prevents unloading and runs outside any frame |
| PILL0304 | PillStaticState       | Error    | A GC handle prevents the project assembly from unloading      |
| PILL0305 | PillStaticState       | Warning  | A finalizer delays assembly unloading                         |
| PILL0401 | PillComponentLayout   | Error    | Unsupported component layout                                  |
| PILL0402 | PillComponentLayout   | Error    | StructLayout.Pack is not honoured for components              |
| PILL0403 | PillComponentLayout   | Error    | Unsupported component field type                              |
