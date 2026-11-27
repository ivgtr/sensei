//! Provider presets and wire formats. Credentials remain scoped to exact API bases.
use anyhow::{bail,Result};
use clap::ValueEnum;
use reqwest::RequestBuilder;
use serde_json::{json,Value};
use std::io::{self,Write};

#[derive(Clone,Copy,Debug,PartialEq,Eq,ValueEnum)]
pub enum Provider { Openai, Gemini, Anthropic, Openrouter, Custom }
impl Provider {
    pub fn base(self)->Option<&'static str>{match self{
        Self::Openai=>Some("https://api.openai.com/v1"),
        Self::Gemini=>Some("https://generativelanguage.googleapis.com/v1beta/openai"),
        Self::Anthropic=>Some("https://api.anthropic.com/v1"),
        Self::Openrouter=>Some("https://openrouter.ai/api/v1"),
        Self::Custom=>None,
    }}
    pub fn from_base(base:&str)->Self{
        [Self::Openai,Self::Gemini,Self::Anthropic,Self::Openrouter].into_iter()
            .find(|p|p.base()==Some(base.trim_end_matches('/'))).unwrap_or(Self::Custom)
    }
    pub fn label(self)->&'static str {match self{
        Self::Openai=>"OpenAI",Self::Gemini=>"Google Gemini",Self::Anthropic=>"Anthropic Claude",
        Self::Openrouter=>"OpenRouter",Self::Custom=>"Custom OpenAI-compatible",
    }}
    pub fn env_key(self)->Option<&'static str>{match self{
        Self::Openai=>Some("OPENAI_API_KEY"),Self::Gemini=>Some("GEMINI_API_KEY"),
        Self::Anthropic=>Some("ANTHROPIC_API_KEY"),Self::Openrouter=>Some("OPENROUTER_API_KEY"),
        Self::Custom=>None,
    }}
    pub fn key_url(self)->Option<&'static str>{match self{
        Self::Openai=>Some("https://platform.openai.com/api-keys"),
        Self::Gemini=>Some("https://aistudio.google.com/apikey"),
        Self::Anthropic=>Some("https://console.anthropic.com/settings/keys"),
        Self::Openrouter=>Some("https://openrouter.ai/keys"),Self::Custom=>None,
    }}
    pub fn presets(self)->&'static [&'static str]{match self{
        Self::Openai=>&["gpt-4.1-mini","gpt-5-mini","gpt-4.1","gpt-5.2","gpt-6.2-astra"],
        Self::Gemini=>&["gemini-3.6-flash","gemini-4-hydrogen","gemini-2.5-flash"],
        Self::Anthropic=>&["claude-sonnet-4-6","claude-haiku-4-5","claude-opus-5"],
        Self::Openrouter=>&["openai/gpt-4.1-mini","google/gemini-3.6-flash","anthropic/claude-sonnet-4.6"],
        Self::Custom=>&[],
    }}
    pub fn default_model(self)->&'static str {self.presets().first().copied().unwrap_or("model-id")}
    pub fn chat_path(self)->&'static str {if self==Self::Anthropic{"/messages"}else{"/chat/completions"}}
    pub fn auth(self,request:RequestBuilder,key:&str)->RequestBuilder{
        if self==Self::Anthropic{request.header("x-api-key",key).header("anthropic-version","2023-06-01")}
        else{request.bearer_auth(key)}
    }
    pub fn request(self,model:&str,system:&str,text:&str,image:Option<&str>)->Value{
        let mut content=vec![json!({"type":"text","text":text})];
        if self==Self::Anthropic {
            if let Some(img)=image{content.push(json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":img}}));}
            // The mentor prompt asks for JSON; the strict local parser is authoritative.
            return json!({"model":model,"system":system,"max_tokens":1024,"messages":[{"role":"user","content":content}]});
        }
        if let Some(img)=image{content.push(json!({"type":"image_url","image_url":{"url":format!("data:image/png;base64,{img}")}}));}
        let mut body=json!({"model":model,"messages":[{"role":"system","content":system},{"role":"user","content":content}],"response_format":{"type":"json_object"}});
        if self==Self::Openai || self==Self::Openrouter{body["max_completion_tokens"]=json!(1024);}
        else {body["max_tokens"]=json!(1024);}
        body
    }
    pub fn response_text(self,result:&Value)->Option<String>{
        if self==Self::Anthropic {
            let parts=result.get("content")?.as_array()?;
            let text=parts.iter().filter(|p|p.get("type").and_then(Value::as_str)==Some("text"))
                .filter_map(|p|p.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n");
            if text.is_empty(){None}else{Some(text)}
        }else{result.pointer("/choices/0/message/content")?.as_str().map(str::to_owned)}
    }
}

fn line(prompt:&str)->Result<String>{
    print!("{prompt}");io::stdout().flush()?;
    let mut s=String::new();if io::stdin().read_line(&mut s)?==0{bail!("Input closed");}Ok(s.trim().to_owned())
}
pub fn select(explicit:Option<Provider>,api_base:Option<String>,current:Provider,noninteractive:bool)->Result<(Provider,String)>{
    if let Some(base)=api_base{
        let base=base.trim_end_matches('/').to_owned();
        let detected=Provider::from_base(&base);
        if explicit.is_some_and(|p|p!=detected){bail!("Provider and API base disagree; use custom for a custom HTTPS endpoint");}
        return Ok((detected,base));
    }
    let chosen=if let Some(p)=explicit{p}else if noninteractive{current}else{
        let choices=[Provider::Openai,Provider::Gemini,Provider::Anthropic,Provider::Openrouter,Provider::Custom];
        println!("APIプロバイダーを選択 (現在: {})",current.label());
        for (i,p) in choices.iter().enumerate(){println!("  {}) {}",i+1,p.label());}
        loop{
            let s=line("番号 / Enterで現在のプロバイダー: ")?;
            if s.is_empty(){break current;}
            if let Ok(n)=s.parse::<usize>(){if let Some(p)=n.checked_sub(1).and_then(|n|choices.get(n)){break *p;}}
            eprintln!("1〜5を選択してください。");
        }
    };
    if let Some(base)=chosen.base(){Ok((chosen,base.into()))}
    else if noninteractive{bail!("Custom provider requires --api-base")}
    else{
        let base=line("OpenAI互換 HTTPS API base: ")?.trim_end_matches('/').to_owned();
        Ok((Provider::from_base(&base),base))
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]fn endpoints_are_exact(){
        for p in [Provider::Openai,Provider::Gemini,Provider::Anthropic,Provider::Openrouter]{
            assert_eq!(Provider::from_base(p.base().unwrap()),p);
        }
        assert_eq!(Provider::from_base("https://api.anthropic.com.evil.example/v1"),Provider::Custom);
        assert!(select(Some(Provider::Gemini),Some("https://api.openai.com/v1".into()),Provider::Openai,true).is_err());
    }
    #[test]fn anthropic_wire(){
        let p=Provider::Anthropic;let b=p.request("claude","sys","question",Some("aGVsbG8="));
        assert_eq!(p.chat_path(),"/messages");
        assert_eq!(b.pointer("/messages/0/content/1/source/media_type").unwrap(),"image/png");
        assert_eq!(b["system"],"sys");
        assert_eq!(p.response_text(&json!({"content":[{"type":"thinking","thinking":"hidden"},{"type":"text","text":"ok"}]})).unwrap(),"ok");
        let req=p.auth(reqwest::Client::new().get("https://api.anthropic.com/v1/models"),"test-key").build().unwrap();
        assert_eq!(req.headers()["x-api-key"],"test-key");
        assert!(!req.headers().contains_key("authorization"));
    }
    #[test]fn compatible_wire(){
        for p in [Provider::Openai,Provider::Gemini,Provider::Openrouter,Provider::Custom]{
            let b=p.request("m","sys","q",Some("abc"));
            assert_eq!(b.pointer("/messages/1/content/1/image_url/url").unwrap(),"data:image/png;base64,abc");
            assert_eq!(p.chat_path(),"/chat/completions");
            assert_eq!(p.response_text(&json!({"choices":[{"message":{"content":"ok"}}]})).unwrap(),"ok");
            let req=p.auth(reqwest::Client::new().get("https://example.org/models"),"key").build().unwrap();
            assert_eq!(req.headers()["authorization"],"Bearer key");
        }
    }
}
