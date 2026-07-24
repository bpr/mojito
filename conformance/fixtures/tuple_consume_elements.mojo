def main():
    var values = ([1, 2, 3], [4, 5, 6])

    @parameter
    def print_length[index: Int](var element: values.element_types[index]):
        print(len(element))

    values^.consume_elements[print_length]()
