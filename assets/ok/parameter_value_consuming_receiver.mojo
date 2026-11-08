# A memory-only compile-time parameter value is an owned temporary: a
# consuming method (`or_else` takes `deinit self`) receives it without a
# copy, and a truth test reads it in storage, here in the lifted bodies of
# local `comptime` bindings. At run time the value materializes into a
# typed temporary, which the native backend lowers as any local.
def note_text[note: Optional[String] = None]() -> String:
    comptime text = String(" note: ", note.or_else(""))
    return text


def op_text[operation: Optional[String] = None]() -> String:
    comptime text = String(" operation: ", operation.value(), ".") if operation else String(
        " this operation."
    )
    return text


def run_note[note: Optional[String] = None]():
    if note:
        print(note.value())
    var v = note.or_else("none")
    var seen = True if note else False
    print(v, seen)


def run_count[count: Optional[Int] = None]():
    if count:
        print(count.value())
    else:
        print("no count")


def main():
    print(note_text[String("hi")]())
    print(note_text())
    print(op_text[String("EBOGUS")]())
    print(op_text())
    run_note[String("b")]()
    run_note()
    run_count[3]()
    run_count()
