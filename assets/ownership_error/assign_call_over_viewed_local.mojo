# expect: conflicts with live reference
# Assigning a call straight back to the local its temporary view argument
# borrows: the view's loan lives for the statement, so the store conflicts
# with it. The pinned Mojo rejects the same program ("aliasing values passed
# immutably to 'v' argument and constructed as a result in 'takes' call").
def takes(v: StringSpan) -> String:
    return String(v)


def main():
    var head = String("/usr/lib")
    head = takes(head[byte=1:4])
    print(head)
