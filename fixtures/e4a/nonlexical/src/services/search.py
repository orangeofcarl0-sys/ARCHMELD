"""Search page size; unrelated to the storefront discount."""

PAGE_SIZE = 90


def page_offset(page: int) -> int:
    return page * PAGE_SIZE
