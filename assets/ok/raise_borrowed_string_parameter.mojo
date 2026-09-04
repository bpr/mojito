# A raising function's propagation path must not free a borrowed
# heap-owning parameter: the caller still owns `w` after each raise.
def boom(s: String, n: Int) raises -> Int:
    if n > 0:
        raise Error("bad " + s)
    return s.byte_length()

def main():
    var w = String("abc")
    try:
        print(boom(w, 1))
    except e:
        print("error:", e)
    try:
        print(boom(w, 2))
    except e:
        print("error:", e)
    try:
        print(boom(w, 0))
    except e:
        print("error:", e)
    print(w)
