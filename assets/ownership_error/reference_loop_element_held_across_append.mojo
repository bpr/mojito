# expect: invalidated interior reference
# A yielded element reference is invalidated by a structural mutation of its
# source: appending reallocates the storage the handle points into, so the
# later use of the binding is rejected.
def main():
    var values = [1, 2, 3]
    for ref x in values:
        values.append(4)
        print(x)
