# Assigning a call straight back to the String whose owned bytes one of its
# arguments borrows: `head[byte=1:4]` is a view at
# `origin_of(head)._get_owned_interior["bytes"]`, so the call's result would
# replace the storage its argument still reads. The pinned Mojo rejects the
# same program with the same text.
# expect: aliasing values passed immutably to 'v' argument and constructed as a result in 'takes' call
def takes(v: StringSpan) -> String:
    return String(v)


def main():
    var head = String("/usr/lib")
    head = takes(head[byte=1:4])
    print(head)
