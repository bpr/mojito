def main():
    var imm_borrowed = [1]
    for item in imm_borrowed:
        print("imm borrowed", item)

    var var_borrowed = [2]
    for var item in var_borrowed:
        item += 10
        print("var borrowed", item)
    print("var source", var_borrowed[0])

    var ref_borrowed = [3]
    for ref item in ref_borrowed:
        item += 10
        print("ref borrowed", item)
    print("ref source", ref_borrowed[0])

    var imm_consumed = [4]
    for item in imm_consumed^:
        print("imm consumed", item)

    var var_consumed = [5]
    for var item in var_consumed^:
        item += 10
        print("var consumed", item)

    var ref_consumed = [6]
    for ref item in ref_consumed^:
        item += 10
        print("ref consumed", item)
