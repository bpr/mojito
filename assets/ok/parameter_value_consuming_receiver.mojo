# A memory-only compile-time parameter value is an owned temporary: a
# consuming method (`or_else` takes `deinit self`) receives it without a
# copy, and a truth test reads it in storage, here in the lifted bodies of
# local `comptime` bindings.
def note_text[note: Optional[String] = None]() -> String:
    comptime text = String(" note: ", note.or_else(""))
    return text


def op_text[operation: Optional[String] = None]() -> String:
    comptime text = String(" operation: ", operation.value(), ".") if operation else String(
        " this operation."
    )
    return text


def main():
    print(note_text[String("hi")]())
    print(note_text())
    print(op_text[String("EBOGUS")]())
    print(op_text())
