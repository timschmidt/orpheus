# Books

Books reads TXT, Markdown, FB2 and EPUB books from the card and keeps your
place in each one. How to read, bookmarks and the reading settings are on
[Reading a book](books-reading.md).

![Library](../shots/books_library_dark.png)

Books has been on the player since 2026-11-27; the automatic tests open
the library and turn the pages of a text book there. The known issues are
below.

## The library

The library lists the books in the `/Books` folder and its folders (eight
levels deep), at most 200. Each row shows the title (from the book, else
the file name) and how far you have read, or **New**.

| Menu entry | What it does |
|---|---|
| Search again | Looks for books again |
| Find books in | **Books folder** or **Whole card** (hidden folders and `/.orpheus` are skipped) |

Without books the library says "No books found" and what to do. OK opens a
book where you left it; Back closes it again and keeps the place.

## Formats

| Format | Files | Notes |
|---|---|---|
| Plain text | `.txt`, `.text` | UTF-8, UTF-16, Windows-1251, KOI8-R and Latin-1 are recognised; lines such as "Chapter 2" become headings |
| Markdown | `.md`, `.markdown` | Headings, quotes, lists, bold and italic; no tables |
| FB2 | `.fb2` | Sections, epigraphs, poems, notes |
| EPUB | `.epub` | EPUB 2 and 3: text, headings and contents; pictures are left out |

The first time a book opens it is prepared ("Preparing the book") into a
plain copy in `/.orpheus/cache/books/` on the card; pages show as soon as
its text is ready, and later opens are quick. A damaged part of an EPUB
shows as a short note in the text, and reading goes on. Copy-protected
(DRM) EPUBs do not read correctly.

## While music plays

Music keeps playing while you read, with these known issues:

* **An EPUB needs the music stopped the first time it opens.** Preparing
  it uses memory that a loaded track holds, so the reader shows "Stop the
  music to prepare this book." Pausing is not enough. Once prepared, the
  book opens while music plays.
* Opening a book while music plays can cause a short dropout.
* Once a book did not open while music played ("The book could not be
  read."); opening it again worked.

## Books on Home

Books is on Home by default. On a player whose Home screen setting was
saved before Books existed, Books is missing: switch it on in
[Settings](settings.md) › Display › Home screen.

## Not yet

**In progress**: Read aloud (the player reading a book to you), pictures
in EPUBs, hyphenation, and Markdown tables.
