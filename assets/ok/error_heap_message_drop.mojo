# An `Error` over a heap message, a local or held in a `List`, drops its
# message's reference to the shared buffer, and a handler without a binder
# drops the raised message, a parameter's copy or a fresh one.
def heap_msg() -> String:
    var m = String(capacity_bytes=40)
    m += "a heap message longer than twenty-three bytes"
    return m^


def errors_over_heap_messages():
    var e = Error(heap_msg())
    print(e)
    var es = List[Error]()
    es.append(Error(heap_msg()))
    print(es[0])


def f(s: String) raises:
    raise s


def g() raises:
    raise Error(String("x") * 30)


def main():
    errors_over_heap_messages()
    var m = String("y") * 30
    for _ in range(3):
        try:
            f(m)
        except:
            pass
        try:
            g()
        except:
            pass
    try:
        f(m)
    except e:
        print(e)
    print("done")
