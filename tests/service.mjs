import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { mkdtempSync, writeFileSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { createServer } from 'node:net';
import { once } from 'node:events';
const binary=resolve(process.env.QMODEMD_BIN || 'target/debug/qmodemd');
async function fixture(level='info', format='json', device='') {
    const dir=mkdtempSync(join(tmpdir(),'qmr-service-'));
    const reservation=createServer();reservation.listen(0,'127.0.0.1');await once(reservation,'listening');
    const port=reservation.address().port;await new Promise(r=>reservation.close(r));
    const config=join(dir,'config.toml');
    writeFileSync(config,`version=1\n[server]\nlisten="127.0.0.1"\nport=${port}\ninterface="${device}"\n[logging]\nlevel="${level}"\nformat="${format}"\n[storage]\nsqlite="${dir}/history.sqlite3"\n`);
    const cli=(...args)=>execFileSync(binary,['--config',config,...args],{encoding:'utf8',stdio:['ignore','pipe','pipe']});
    const token=JSON.parse(cli('init-auth')).token;
    let child, output='', exited;
    return {dir,port,config,token,cli,get logs(){return output;},
        async start() {
            child=spawn(binary,['--config',config,'serve'],{stdio:['ignore','pipe','pipe']});
            exited=once(child,'exit');child.stderr.on('data',chunk=>output+=chunk);child.stdout.on('data',chunk=>output+=chunk);
            for(let i=0;i<100;i++){
                if(child.exitCode!==null)throw new Error('Service exited: '+output);
                try{const r=await fetch(`http://127.0.0.1:${port}/api/health`);if(r.ok)return;}catch{}
                await new Promise(r=>setTimeout(r,20));
            }throw new Error('Service startup timeout: '+output);
        },
        async stop(){if(child && child.exitCode===null){child.kill('SIGTERM');await exited;}},
        async cleanup(){await this.stop();rmSync(dir,{recursive:true,force:true});}
    };
}

test('listener settings, hash-only authentication and persistence',async()=>{
    const f=await fixture();
    try{
        f.cli('set-service','--interface','lo','--log-level','debug','--log-format','json');
        const info=JSON.parse(f.cli('service-info'));
        assert.equal(info.interface,'lo');assert.equal(info.log_level,'debug');assert.equal(info.auth_configured,true);
        assert.ok(!JSON.stringify(info).includes(f.token));
        assert.ok(!readFileSync(f.config,'utf8').includes(f.token));
        await f.start();
        let r=await fetch(`http://127.0.0.1:${f.port}/api/v1/system/service`);assert.equal(r.status,401);
        r=await fetch(`http://127.0.0.1:${f.port}/api/v1/system/service`,{headers:{Authorization:`Bearer ${f.token}`}});
        assert.equal(r.status,200);assert.equal((await r.json()).data.interface,'lo');
        await f.stop();
        assert.ok(!f.logs.includes(f.token));assert.ok(!f.logs.includes('Authorization'));
    }finally{await f.cleanup();}
});

test('every configured severity filters actual service events',async()=>{
    for(const level of ['off','error','warn','info','debug','trace']){
        const f=await fixture(level);
        try{
            await f.start();
            await fetch(`http://127.0.0.1:${f.port}/api/v1/system/service?secret=never-log-this`);
            await f.stop();
            const events=f.logs.trim()?f.logs.trim().split('\n').map(s=>JSON.parse(s)):[];
            const levels=events.map(e=>e.level);
            if(level==='off'||level==='error')assert.equal(events.length,0,level);
            if(['warn','info','debug','trace'].includes(level))assert.ok(levels.includes('WARN'),level);
            if(['info','debug','trace'].includes(level))assert.ok(levels.includes('INFO'),level);
            if(['debug','trace'].includes(level))assert.ok(levels.includes('DEBUG'),level);
            if(level==='warn')assert.ok(levels.every(l=>l==='WARN'||l==='ERROR'));
            assert.ok(!f.logs.includes('never-log-this'));
        }finally{await f.cleanup();}
    }
});

test('bad interface fails closed and a bad patch keeps the original TOML',async()=>{
    const f=await fixture();
    try{
        const before=readFileSync(f.config,'utf8');
        assert.throws(()=>f.cli('set-service','--interface','../lo'));
        assert.equal(readFileSync(f.config,'utf8'),before);
        f.cli('set-service','--interface','qmr-missing');
        assert.throws(()=>f.cli('serve'),/does not exist/);
        f.cli('set-service','--interface','any');
        assert.equal(JSON.parse(f.cli('service-info')).interface,'');
    }finally{await f.cleanup();}
});

test('non-loopback listening requires authentication before opening socket',async()=>{
    const f=await fixture();
    try{
        let cfg=readFileSync(f.config,'utf8').replace('127.0.0.1','0.0.0.0').replace(/token_hash = "[a-f0-9]+"/,'token_hash = ""');
        writeFileSync(f.config,cfg);
        assert.throws(()=>f.cli('serve'),/requires an access token/);
    }finally{await f.cleanup();}
});
