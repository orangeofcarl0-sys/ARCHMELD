"""Historical pricing archive; ARCHIVE_LIMIT is an unrelated row count."""

ARCHIVE_LIMIT = 11110


def keep(index: int) -> bool:
    return index < ARCHIVE_LIMIT
