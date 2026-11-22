M=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/M1
cd $M
python3 mutcount2.py E2 src/ordindex.rs '        let a = if hi <= held.len() {\n' '        let a = if hi < held.len() {\n' land 'Reader>::cut_raw' 'OrdIndex>::seek_in' 'OrdIndex>::head_absent' || echo E2 FAILED
python3 mutcount2.py E6 src/ordindex.rs '        if self.lim == 0 {\n            return &[];\n        }\n        let p = self.heads.load' '        if true {\n            return &[];\n        }\n        let p = self.heads.load' land 'Reader>::cut_raw' 'OrdIndex>::seek_in' 'OrdIndex>::head_absent' || echo E6 FAILED
python3 mutcount2.py E3b src/db.rs '            if m.quiet_out.get() {\n                m.quiet_out.set(false);\n                self.shared\n                    .write_quiet\n' '            let was = m.quiet_out.get();\n            m.quiet_out.set(false);\n            if was {\n                self.shared\n                    .write_quiet\n' scan 'Reader>::scan<' || echo E3b FAILED
echo EXIT done
