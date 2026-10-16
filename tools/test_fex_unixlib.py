"""Optional native companion ABI checks; never starts Wine or Steam."""
import ctypes
import os
import tempfile
import unittest


@unittest.skipUnless(os.environ.get('NUCLEON_TEST_FEX_UNIX_LIBRARY'), 'requires a built Darwin companion')
class UnixLibTests(unittest.TestCase):
    def test_pinned_function_table_and_file_mapping(self):
        library = ctypes.CDLL(os.environ['NUCLEON_TEST_FEX_UNIX_LIBRARY'])
        table = (ctypes.c_void_p * 8).in_dll(library, '__wine_unix_call_funcs')
        call = lambda index, args: ctypes.CFUNCTYPE(ctypes.c_uint32, ctypes.c_void_p)(table[index])(ctypes.byref(args))
        pid = ctypes.c_uint32()
        self.assertEqual(call(7, pid), 0)
        self.assertEqual(pid.value, os.getpid())
        # Linux-only controls/advice must report unsupported on Darwin.
        enabled = ctypes.c_bool(True)
        self.assertEqual(call(0, enabled), 0xc00000bb)
        flags = ctypes.c_uint64(1)
        self.assertEqual(call(1, flags), 0xc00000bb)
        class Advice(ctypes.Structure):
            _fields_ = [('address', ctypes.c_void_p), ('size', ctypes.c_size_t), ('advice', ctypes.c_int32), ('pad', ctypes.c_uint32)]
        self.assertEqual(call(2, Advice(None, 0, 14, 0)), 0xc00000bb)
        class Mapping(ctypes.Structure):
            _fields_ = [('fd', ctypes.c_int32), ('size', ctypes.c_uint64), ('result', ctypes.c_void_p)]
        with tempfile.TemporaryFile() as stream:
            stream.write(b'FEX ABI'); stream.flush()
            fd = os.dup(stream.fileno())
            mapping = Mapping(fd, 7, None)
            self.assertEqual(call(6, mapping), 0)
            try:
                self.assertEqual(ctypes.string_at(mapping.result, 7), b'FEX ABI')
                with self.assertRaises(OSError): os.fstat(fd)  # ownership consumed
            finally:
                libc = ctypes.CDLL(None)
                libc.munmap.argtypes = [ctypes.c_void_p, ctypes.c_size_t]
                self.assertEqual(libc.munmap(mapping.result, 7), 0)
