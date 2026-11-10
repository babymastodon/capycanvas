import importlib.util
from pathlib import Path
import json
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("motion_report", Path(__file__).with_name("windows-ui-motion-report.py"))
report = importlib.util.module_from_spec(spec)
spec.loader.exec_module(report)


class ReportTests(unittest.TestCase):
    def test_process_cpu_per_displayed_change_pools_work_instead_of_inputs(self):
        def contact(cpu,changes,inputs):
            return {'input_count':inputs,'cpu':{'active_ui_ms':1,'process_ms':cpu,
                    'active_process_ms':cpu-1,'private_bytes':1},
                    'probes':{'content':{'changes':changes,'motion_seconds':1,
                    'samples':{key:[] for key in ('interval_ms','interior_interval_ms','response_ms','completion_ms')}}}}
        control=report.pooled([contact(12,1,10),contact(18,2,10)],'content')
        candidate=report.pooled([contact(20,3,10),contact(20,3,10)],'content')
        self.assertGreater(candidate['process_ms_per_input'],control['process_ms_per_input'])
        self.assertLess(candidate['process_ms_per_change'],control['process_ms_per_change'])
        self.assertEqual(control['process_ms_per_change'],10)
        self.assertEqual(control['active_process_ms_per_change'],28/3)
        empty=report.pooled([contact(1,0,1)],'content')
        self.assertIsNone(empty['process_ms_per_change'])
        self.assertIsNone(empty['active_process_ms_per_change'])

    def test_motion_cadence_separates_turn_dwell_and_preserves_mid_run_stalls(self):
        inputs=[{'qpc':0,'expected_value':0,'segment':0},
                {'qpc':130,'expected_value':100,'segment':0},
                {'qpc':280,'expected_value':100,'segment':1},
                {'qpc':400,'expected_value':0,'segment':1}]
        frames=[{'qpc':t,'position':value} for t,value in
                ((0,10),(10,20),(110,30),(120,90),(130,100),(280,100),(290,90),(300,80))]
        result=report.interior_motion_cadence(frames,inputs,'position',1000)
        self.assertTrue(result['qualified'])
        self.assertEqual(result['interval_ms'],[100,10,10])
        self.assertEqual(result['observed_turns'],1)
        frames[-3]['position']=70
        result=report.interior_motion_cadence(frames,inputs,'position',1000)
        self.assertFalse(result['qualified'])

    def test_ideal_motion_has_long_raw_intervals_at_rounded_turnaround(self):
        states=((0,0,0),(10,25,0),(20,50,0),(30,75,0),(40,100,0),
                (190,100,1),(200,75,1),(210,50,1),(220,25,1),(230,0,1))
        frames=[{'qpc':t,'position':value} for t,value,_ in states]
        inputs=[{'qpc':t,'expected_value':value,'segment':segment} for t,value,segment in states]
        stamps=report.changes(frames,'position',0,230)
        self.assertGreater(max(b-a for a,b in zip(stamps,stamps[1:])),100)
        result=report.interior_motion_cadence(frames,inputs,'position',1000)
        self.assertTrue(result['qualified'])
        self.assertEqual(report.quantiles(result['interval_ms'])['p99'],10)

    def test_faster_response_exposes_more_of_the_same_intentional_dwell(self):
        states=((0,0,0),(10,25,0),(20,50,0),(30,75,0),(40,100,0),
                (190,100,1),(200,75,1),(210,50,1),(220,25,1),(230,0,1))
        inputs=[{'qpc':t,'expected_value':value,'segment':segment} for t,value,segment in states]
        fast=[{'qpc':t,'position':value} for t,value,_ in states]
        slow=[{'qpc':t+(80 if segment==0 else 40),'position':value} for t,value,segment in states]
        def raw_p99(frames):
            stamps=report.changes(frames,'position',0,1000)
            return report.quantiles(b-a for a,b in zip(stamps,stamps[1:]))['p99']
        self.assertGreater(raw_p99(fast),raw_p99(slow))
        self.assertLess(report.quantiles(f['qpc']-sample['qpc'] for f,sample in zip(fast,inputs))['p50'],
                        report.quantiles(f['qpc']-sample['qpc'] for f,sample in zip(slow,inputs))['p50'])
        for frames in (fast,slow):
            result=report.interior_motion_cadence(frames,inputs,'position',1000)
            self.assertTrue(result['qualified'])
            self.assertEqual(report.quantiles(result['interval_ms'])['p99'],10)

    def test_inverted_motion_cannot_qualify_with_the_expected_number_of_turns(self):
        inputs=[{'qpc':t,'expected_value':value,'segment':segment} for t,value,segment in
                ((0,50,0),(10,100,0),(20,100,1),(30,0,1),(40,0,2),(50,50,2))]
        frames=[{'qpc':10*i,'position':value} for i,value in enumerate((50,25,0,25,50,75,100,75,50))]
        result=report.interior_motion_cadence(frames,inputs,'position',1000)
        self.assertEqual(result['observed_turns'],result['expected_turns'])
        self.assertFalse(result['direction_match'])
        self.assertFalse(result['qualified'])

    def test_interior_cadence_requires_every_contact_without_changing_raw_intervals(self):
        def contact(qualified):
            return {'input_count':1,'cpu':{'active_ui_ms':1,'process_ms':1,'active_process_ms':1,'private_bytes':1},
                    'probes':{'edge':{'changes':1,'motion_seconds':1,'samples':{'interval_ms':[7],
                        'interior_interval_ms':[10],'response_ms':[],'completion_ms':[]},
                        'interior_motion_cadence':{'qualified':qualified}}}}
        result=report.pooled([contact(True),contact(False)],'edge')
        self.assertIsNone(result['interior_interval_ms_p99'])
        self.assertEqual(result['interval_ms_p99'],7)

    def test_typing_caret_changes_without_field_targets_do_not_qualify(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)
            def save(name,value):
                (root/name).write_text(json.dumps(value))
            def lines(name,rows):
                (root/name).write_text(''.join(json.dumps(row)+'\n' for row in rows))
            save('plan.json',{'axis':0,'point':[0,0],'probes':[{'name':'field','kind':'change'}]})
            save('meta.json',{'qpc_frequency':1000,'idle_start_qpc':0,'motion_start_qpc':1000,
                             'motion_end_qpc':7000,'workload':'typing','refresh_hz':120,'no_capture':False})
            save('cpu.json',[{'qpc':t,'ui_cpu_ms':t/1000,'cpu_ms':t/1000,'private_bytes':4096} for t in (0,1000,7000)])
            lines('inputs.jsonl',[{'qpc':1000+100*i,'slot':i} for i in range(60)])
            lines('frames.jsonl',[{'qpc':t,'last_present_qpc':t,'accumulated_frames':1,'capture_ms':1,
                                  'probes':{'field':{'position':None,'hash':str(t//300%2),'changed_pixels':5,
                                                     'valid':True}}} for t in range(0,7401,10)])
            save('completed.json',{'ok':True})
            result=report.summarize_contact(root)
            self.assertFalse(result['valid'])
            self.assertIn('field: missing calibrated typing states',result['errors'])

    def test_idle_observation_does_not_count_as_a_changed_frame(self):
        frames = [{"last_present_qpc":i+1,"position":p} for i,p in enumerate([4,4,5,5,6])]
        self.assertEqual(report.changes(frames,"position",2,5),[3,5])

    def test_low_amplitude_pixel_noise_does_not_count(self):
        frames=[{"last_present_qpc":1,"hash":1,"changed_pixels":0},{"last_present_qpc":2,"hash":2,"changed_pixels":0},{"last_present_qpc":3,"hash":3,"changed_pixels":5}]
        self.assertEqual(report.visible_changes(frames,"hash",1,3),[3])

    def test_discrete_completion_uses_terminal_pixels_and_separates_events(self):
        frames = [{"last_present_qpc":t,"hash":h,"changed_pixels":n,"valid":True}
                  for t,h,n in [(1,1,5),(3,2,5),(9,2,0),(12,3,5),(15,4,5),(19,4,0)]]
        response,completion,windows,missing,incomplete = report.discrete_latencies(frames,[{"qpc":0},{"qpc":10}],1000,20,[2,4])
        self.assertEqual(response,[1,2])
        self.assertEqual(completion,[3,5])
        self.assertEqual(windows,[(0,3),(10,15)])
        self.assertEqual(missing,0)
        self.assertEqual(incomplete,0)

    def test_partial_animation_cannot_be_a_completion(self):
        frames=[{"last_present_qpc":1,"hash":"halfway","changed_pixels":10,"valid":True}]
        response,completion,windows,missing,incomplete=report.discrete_latencies(frames,[{"qpc":0}],1000,20,["open"])
        self.assertEqual(response,[1]);self.assertEqual(completion,[]);self.assertEqual(windows,[]);self.assertEqual(incomplete,1)

    def test_cpu_interpolates_boundaries_and_subtracts_idle(self):
        rows = [{"qpc":t,"ui_cpu_ms":cpu,"cpu_ms":2*cpu,"private_bytes":4096} for t,cpu in [(0,0),(10,1),(20,6),(30,11)]]
        meta = {"motion_start_qpc":10,"motion_end_qpc":30,"idle_start_qpc":0,"qpc_frequency":10}
        result=report.cpu_usage(rows,meta)
        self.assertEqual(result["active_ui_ms"],8)
        self.assertEqual(result["process_ms"],20)
        self.assertEqual(result["active_process_ms"],16)
        self.assertEqual(report.interpolate(rows,15,"ui_cpu_ms"),3.5)
        with self.assertRaises(ValueError):
            report.interpolate(rows,31,"ui_cpu_ms")

    def test_aa_noise_and_control_drift_block_small_improvement(self):
        slots=[{"arm":arm,"values":[v],"aa_noise":2} for arm,v in zip(["control","candidate","candidate","control"],[10,9,9,11])]
        self.assertFalse(report.compare_abba(slots)["improves_lower"])
        slots[1]["values"]=slots[2]["values"]=[6]
        self.assertTrue(report.compare_abba(slots)["improves_lower"])
        slots[0]["arm"]="candidate"
        with self.assertRaises(ValueError):
            report.compare_abba(slots)

    def test_drag_latency_never_matches_a_later_injection(self):
        inputs=[{"qpc":0,"expected_value":0,"segment":0},{"qpc":10,"expected_value":10,"segment":0},{"qpc":20,"expected_value":20,"segment":0}]
        frames=[{"last_present_qpc":5,"position":10},{"last_present_qpc":15,"position":10}]
        values,_,unmatched=report.latency_drag(frames,inputs,"position",1000)
        self.assertEqual(values,[5])
        self.assertEqual(unmatched,1)

    def test_pen_only_aa_has_metric_noise_and_requires_every_contact(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)
            (root/'order.json').write_text(json.dumps({"order":"aa","slots":[["control","a"],["candidate","a"],["candidate","a"],["control","a"]],"workloads":["pen"]}))
            rows=[]
            for i in range(4):
                contact={"renderer_frames_per_s":100+i,"ui_ms_per_input":1.,"ui_ms_per_renderer_change":2.,"private_bytes":1024,
                         "interval_ms":{"p50_ms":10.,"p95_ms":11.,"p99_ms":12.},"response_ms":{"p50_ms":2.,"p95_ms":3.,"p99_ms":4.}}
                rows.append({"path":str(root/f'{i}-control-pen'/'pen'),"contacts":[contact.copy() for _ in range(3)]})
            actual=report.aggregate(root,[],pen=rows)
            self.assertEqual(actual["errors"],[])
            self.assertEqual(actual["workloads"]["pen/renderer"]["metrics"]["renderer_frames_per_s"]["noise"],3)
            rows[0]["contacts"].pop()
            self.assertTrue(report.aggregate(root,[],pen=rows)["errors"])


if __name__ == "__main__":
    unittest.main()
