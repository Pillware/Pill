"""
The shape of one rendered engine log line.

REQUIREMENTS: Python 3.8+.

DESCRIPTION
    The engine's formatter writes one line as

        [hh:mm:ss:mmm] LEVEL target  message | field=value field=value

    The suites match those lines, so the layout is described once here. A
    suite that needs a message-then-fields pattern builds it with
    `message_then_fields_pattern` instead of writing the gap itself; a token
    compared as plain text with `wait_for` uses `message_then_fields`.

    The grammar:

      time      optional `[hh:mm:ss:mmm]` or `[dd.mm.yyyy hh:mm:ss:mmm]`
      level     TRACE, DEBUG, INFO, WARN or ERROR, padded to five columns
      target    the tracing target, such as `engine::hot_reload`
      message   fixed lowercase words, no trailing period
      separator ` | `, present only when the line has fields
      fields    `name=value` pairs separated by whitespace
      location  optional trailing `file:line`

    A line with no fields has no separator. A block's heading stays on the
    prefix line before its fields, and its body follows below.

--- SCRIPT ---
"""

SEPARATOR = " | "
# The separator as a regex. A rendered line may pad it with spaces on either
# side; the pipe itself is the fixed part.
SEPARATOR_PATTERN = r"\s+\|\s+"
# The gap between two fields on one line.
FIELD_GAP_PATTERN = r"\s+"


def message_then_fields(message: str, fields: str) -> str:
    """Returns `message` and `fields` joined as the formatter renders them.

    Use this for a token compared as plain text with `wait_for`: both parts
    are already the complete text of that segment of the line.
    """
    return f"{message}{SEPARATOR}{fields}"


def message_then_fields_pattern(message_pattern: str, fields_pattern: str) -> str:
    """Returns a regex for a message pattern followed by a fields pattern.

    `message_pattern` is already escaped, or a deliberate pattern; the two
    parts are joined through the one separator pattern this module owns. A
    fields pattern may keep the ``\\s+`` that used to separate the message
    from its first field: the gap is folded into the separator, so both
    spellings build the same regex.
    """
    if fields_pattern.startswith(FIELD_GAP_PATTERN):
        fields_pattern = fields_pattern[len(FIELD_GAP_PATTERN) :]
    return f"{message_pattern}{SEPARATOR_PATTERN}{fields_pattern}"
